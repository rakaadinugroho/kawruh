//! DocHub: the Rust side owns every document.
//!
//! Webviews hold mirror `Y.Doc`s connected through IPC "views". Every update,
//! whatever its source, lands here once (observer → event loop), is persisted,
//! then routed by origin: to the other views, and to the server unless it came
//! from the server.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use kawruh_core::store::{self, DocKind};
use kawruh_core::sync_client::ORIGIN_REMOTE;
use kawruh_core::yrs::{Doc, ReadTxn, StateVector, Transact};
use rusqlite::Connection;
use serde::Serialize;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

use crate::error::{Error, Result};

/// Origin of edits made by Rust itself (import, onboarding content).
pub const ORIGIN_LOCAL: &str = "local";

pub fn view_origin(view: u32) -> String {
    format!("view:{view}")
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DocEvent {
    Update { update: String },
    Awareness { update: String },
}

struct Entry {
    doc: Doc,
    workspace_id: String,
    views: HashMap<u32, Channel<DocEvent>>,
}

pub struct Routed {
    /// The update must also be sent to the server.
    pub to_server: bool,
    /// Saving it locally failed (shown to the user; retried later).
    pub persist_error: Option<String>,
}

pub struct HubEvent {
    pub doc_id: String,
    pub workspace_id: String,
    pub update: Vec<u8>,
    pub origin: Option<String>,
}

pub struct DocHub {
    pub db: Arc<Mutex<Connection>>,
    entries: Mutex<HashMap<String, Entry>>,
    /// Docs whose last update failed to persist; compacted (full state) once
    /// writes succeed again so nothing is lost on restart.
    unsaved: Mutex<HashSet<String>>,
    next_view: AtomicU32,
    events: mpsc::UnboundedSender<HubEvent>,
}

impl DocHub {
    pub fn new(db: Arc<Mutex<Connection>>) -> (Arc<DocHub>, mpsc::UnboundedReceiver<HubEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let hub = Arc::new(DocHub {
            db,
            entries: Mutex::new(HashMap::new()),
            unsaved: Mutex::new(HashSet::new()),
            next_view: AtomicU32::new(1),
            events: tx,
        });
        (hub, rx)
    }

    /// Returns the live document, loading it from SQLite (or creating it).
    pub fn get_or_load(&self, doc_id: &str, workspace_id: &str) -> Result<Doc> {
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.get(doc_id) {
            // A doc id from one workspace must never be served to another
            // (it could leak content to, or take edits from, another server).
            if e.workspace_id != workspace_id {
                return Err(Error::msg(format!(
                    "document {doc_id} belongs to another workspace"
                )));
            }
            return Ok(e.doc.clone());
        }
        let doc = {
            let db = self.db.lock().unwrap();
            if let Some(owner) = store::doc_workspace(&db, doc_id)?
                && owner != workspace_id
            {
                return Err(Error::msg(format!(
                    "document {doc_id} belongs to another workspace"
                )));
            }
            let kind = if doc_id == workspace_id {
                DocKind::Workspace
            } else {
                DocKind::Page
            };
            store::ensure_doc(&db, doc_id, workspace_id, kind)?;
            store::load_doc(&db, doc_id)?.unwrap_or_default()
        };
        let tx = self.events.clone();
        let (id, ws) = (doc_id.to_string(), workspace_id.to_string());
        doc.observe_update_v1("hub", move |txn, e| {
            let origin = txn
                .origin()
                .map(|o| String::from_utf8_lossy(o.as_ref()).to_string());
            let _ = tx.send(HubEvent {
                doc_id: id.clone(),
                workspace_id: ws.clone(),
                update: e.update.clone(),
                origin,
            });
        })
        .map_err(|e| Error::msg(e.to_string()))?;
        entries.insert(
            doc_id.to_string(),
            Entry {
                doc: doc.clone(),
                workspace_id: workspace_id.to_string(),
                views: HashMap::new(),
            },
        );
        Ok(doc)
    }

    pub fn doc(&self, doc_id: &str) -> Option<Doc> {
        self.entries
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|e| e.doc.clone())
    }

    /// Attaches a webview mirror. Returns its view id and the full state.
    pub fn open_view(
        &self,
        doc_id: &str,
        workspace_id: &str,
        channel: Channel<DocEvent>,
    ) -> Result<(u32, Vec<u8>)> {
        self.get_or_load(doc_id, workspace_id)?;
        let view = self.next_view.fetch_add(1, Ordering::Relaxed);
        // Snapshot and register under one lock: `route` needs this lock to
        // deliver, so every update not in the snapshot reaches the new view.
        let mut entries = self.entries.lock().unwrap();
        let e = entries
            .get_mut(doc_id)
            .ok_or_else(|| Error::msg(format!("document {doc_id} was unloaded")))?;
        let state = e
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        e.views.insert(view, channel);
        Ok((view, state))
    }

    pub fn close_view(&self, doc_id: &str, view: u32) {
        if let Some(e) = self.entries.lock().unwrap().get_mut(doc_id) {
            e.views.remove(&view);
        }
    }

    pub fn apply_from_view(&self, doc_id: &str, view: u32, update: &[u8]) -> Result<()> {
        let doc = self
            .doc(doc_id)
            .ok_or_else(|| Error::msg(format!("document {doc_id} is not open")))?;
        kawruh_core::protocol::apply_update(&doc, update, &view_origin(view))
            .map_err(|e| Error::msg(e.to_string()))
    }

    pub fn deliver_awareness(&self, doc_id: &str, update: &[u8]) {
        let entries = self.entries.lock().unwrap();
        if let Some(e) = entries.get(doc_id) {
            let event = DocEvent::Awareness {
                update: B64.encode(update),
            };
            for ch in e.views.values() {
                let _ = ch.send(event.clone());
            }
        }
    }

    /// Drops every loaded document of a workspace (after it is forgotten).
    pub fn unload_workspace(&self, workspace_id: &str) {
        self.entries
            .lock()
            .unwrap()
            .retain(|_, e| e.workspace_id != workspace_id);
    }

    /// Persists an update and forwards it to the other views.
    pub fn route(&self, ev: &HubEvent) -> Routed {
        let appended = {
            let db = self.db.lock().unwrap();
            store::append_update(&db, &ev.doc_id, &ev.update)
        };
        let mut persist_error = None;
        let pending = match appended {
            Ok(n) => n,
            Err(e) => {
                tracing::error!("persist {} failed: {e}", ev.doc_id);
                persist_error = Some(e.to_string());
                self.unsaved.lock().unwrap().insert(ev.doc_id.clone());
                0
            }
        };
        let entries = self.entries.lock().unwrap();
        if persist_error.is_none() {
            // Writes work again: fold previously failed docs into snapshots.
            let retry: Vec<String> = self.unsaved.lock().unwrap().iter().cloned().collect();
            for id in retry {
                if let Some(e) = entries.get(&id) {
                    let mut db = self.db.lock().unwrap();
                    if store::compact(&mut db, &id, &e.doc).is_ok() {
                        self.unsaved.lock().unwrap().remove(&id);
                    }
                }
            }
        }
        if let Some(e) = entries.get(&ev.doc_id) {
            if pending > store::COMPACT_AFTER {
                let mut db = self.db.lock().unwrap();
                if let Err(err) = store::compact(&mut db, &ev.doc_id, &e.doc) {
                    tracing::error!("compact {} failed: {err}", ev.doc_id);
                }
            }
            let skip = ev
                .origin
                .as_deref()
                .and_then(|o| o.strip_prefix("view:"))
                .and_then(|v| v.parse::<u32>().ok());
            let event = DocEvent::Update {
                update: B64.encode(&ev.update),
            };
            for (view, ch) in &e.views {
                if Some(*view) != skip {
                    let _ = ch.send(event.clone());
                }
            }
        }
        Routed {
            to_server: ev.origin.as_deref() != Some(ORIGIN_REMOTE),
            persist_error,
        }
    }
}
