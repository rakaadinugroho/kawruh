//! Live documents shared by WebSocket clients, MCP and the share view.
//!
//! Each loaded document has one `observe_update_v1` hook that forwards every
//! update (with its transaction origin) to a single event task, which persists
//! it, relays it to all subscribers except the one it came from, and marks the
//! document dirty for the search index. Idle documents are unloaded.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc};

use kawruh_core::protocol::{self, Frame, Kind};
use kawruh_core::store::{self, DocKind};

use crate::db::{self, Db};

pub type ConnId = u64;
/// Encoded frames destined for one connection.
pub type Outbox = mpsc::UnboundedSender<Vec<u8>>;

pub const ORIGIN_MCP: &str = "mcp";
pub const ORIGIN_SERVER: &str = "server";

pub fn conn_origin(conn: ConnId) -> String {
    format!("conn:{conn}")
}

/// Unload documents nobody has touched for this long.
const IDLE_UNLOAD: Duration = Duration::from_secs(120);
/// Debounce for search-index refreshes.
const INDEX_DEBOUNCE: Duration = Duration::from_secs(2);

pub struct LiveDoc {
    pub id: String,
    pub workspace_id: String,
    pub doc: Doc,
    subscribers: Mutex<HashMap<ConnId, Outbox>>,
    awareness: Mutex<HashMap<ClientID, (ConnId, AwarenessUpdateEntry)>>,
    closed: AtomicBool,
    last_active: Mutex<Instant>,
}

impl LiveDoc {
    pub fn kind(&self) -> DocKind {
        if self.id == self.workspace_id {
            DocKind::Workspace
        } else {
            DocKind::Page
        }
    }

    fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    pub fn add_subscriber(&self, conn: ConnId, outbox: Outbox) {
        self.subscribers.lock().unwrap().insert(conn, outbox);
        self.touch();
    }

    /// Removes a subscriber; returns the awareness update announcing that its
    /// clients left, if it had any.
    pub fn remove_subscriber(&self, conn: ConnId) -> Option<Vec<u8>> {
        self.subscribers.lock().unwrap().remove(&conn);
        self.touch();
        let mut aw = self.awareness.lock().unwrap();
        let gone: Vec<ClientID> = aw
            .iter()
            .filter(|(_, (c, _))| *c == conn)
            .map(|(id, _)| *id)
            .collect();
        if gone.is_empty() {
            return None;
        }
        let mut clients = HashMap::new();
        for id in gone {
            if let Some((_, entry)) = aw.remove(&id) {
                clients.insert(
                    id,
                    AwarenessUpdateEntry {
                        clock: entry.clock + 1,
                        json: "null".into(),
                    },
                );
            }
        }
        Some(AwarenessUpdate { clients }.encode_v1())
    }

    /// Records the awareness states a connection announced.
    pub fn apply_awareness(&self, conn: ConnId, payload: &[u8]) -> bool {
        let Ok(update) = AwarenessUpdate::decode_v1(payload) else {
            return false;
        };
        let mut aw = self.awareness.lock().unwrap();
        for (id, entry) in update.clients {
            if entry.json.as_ref() == "null" {
                aw.remove(&id);
            } else {
                aw.insert(id, (conn, entry));
            }
        }
        true
    }

    pub fn awareness_snapshot(&self) -> Option<Vec<u8>> {
        let aw = self.awareness.lock().unwrap();
        if aw.is_empty() {
            return None;
        }
        let clients = aw.iter().map(|(id, (_, e))| (*id, e.clone())).collect();
        Some(AwarenessUpdate { clients }.encode_v1())
    }

    /// Sends a frame to every subscriber except `except`.
    pub fn broadcast(&self, frame: &Frame, except: Option<ConnId>) {
        let bytes = frame.encode();
        let subs = self.subscribers.lock().unwrap();
        for (conn, outbox) in subs.iter() {
            if Some(*conn) != except {
                let _ = outbox.send(bytes.clone());
            }
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("document belongs to another workspace")]
    Forbidden,
    #[error(transparent)]
    Store(#[from] store::StoreError),
}

struct DocEvent {
    doc_id: String,
    workspace_id: String,
    update: Vec<u8>,
    origin: Option<Vec<u8>>,
}

pub struct DocRegistry {
    db: Db,
    docs: Mutex<HashMap<String, Arc<LiveDoc>>>,
    events: mpsc::UnboundedSender<DocEvent>,
    dirty: mpsc::UnboundedSender<(String, String)>,
    data_dir: PathBuf,
    min_free_bytes: u64,
    disk_ok: AtomicBool,
    disk_checked_ms: AtomicU64,
}

impl DocRegistry {
    pub fn new(db: Db, data_dir: PathBuf, min_free_mb: u64) -> Arc<DocRegistry> {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (dirty, dirty_rx) = mpsc::unbounded_channel();
        let reg = Arc::new(DocRegistry {
            db,
            docs: Mutex::new(HashMap::new()),
            events,
            dirty,
            data_dir,
            min_free_bytes: min_free_mb * 1024 * 1024,
            disk_ok: AtomicBool::new(true),
            disk_checked_ms: AtomicU64::new(0),
        });
        tokio::spawn(event_loop(reg.clone(), events_rx));
        tokio::spawn(index_loop(reg.clone(), dirty_rx));
        tokio::spawn(janitor(Arc::downgrade(&reg)));
        reg
    }

    /// Whether there is enough free disk to accept document writes.
    /// Re-checked at most every 5 seconds.
    pub fn writes_allowed(&self) -> bool {
        let now = kawruh_core::workspace::now_ms() as u64;
        let last = self.disk_checked_ms.load(Ordering::Relaxed);
        if now.saturating_sub(last) > 5_000 {
            self.disk_checked_ms.store(now, Ordering::Relaxed);
            let ok = crate::disk::free_bytes(&self.data_dir)
                .map(|free| free >= self.min_free_bytes)
                .unwrap_or(true);
            if !ok {
                tracing::warn!(
                    "free disk below {} bytes: refusing document writes",
                    self.min_free_bytes
                );
            }
            self.disk_ok.store(ok, Ordering::Relaxed);
        }
        self.disk_ok.load(Ordering::Relaxed)
    }

    /// Returns the live document, loading it from storage when needed.
    /// `register` records an unknown document as belonging to `workspace_id`
    /// right away (editors); otherwise registration happens on first write.
    pub fn get_or_load(
        &self,
        doc_id: &str,
        workspace_id: &str,
        register: bool,
    ) -> Result<Arc<LiveDoc>, DocError> {
        let mut docs = self.docs.lock().unwrap();
        if let Some(live) = docs.get(doc_id) {
            if live.workspace_id != workspace_id {
                return Err(DocError::Forbidden);
            }
            live.touch();
            return Ok(live.clone());
        }
        let kind = if doc_id == workspace_id {
            DocKind::Workspace
        } else {
            DocKind::Page
        };
        let doc = {
            let mut conn = self.db.lock();
            match store::doc_workspace(&conn, doc_id)? {
                Some(ws) if ws != workspace_id => return Err(DocError::Forbidden),
                Some(_) => {
                    let doc = store::load_doc(&conn, doc_id)?.unwrap_or_default();
                    // Fold a long tail of updates into the snapshot on load.
                    let pending: i64 = conn
                        .query_row(
                            "SELECT COUNT(*) FROM doc_updates WHERE doc_id = ?1",
                            [doc_id],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    if pending >= store::COMPACT_AFTER {
                        store::compact(&mut conn, doc_id, &doc)?;
                    }
                    doc
                }
                None => {
                    if register {
                        store::ensure_doc(&conn, doc_id, workspace_id, kind)?;
                    }
                    Doc::new()
                }
            }
        };
        let events = self.events.clone();
        let (id, ws) = (doc_id.to_string(), workspace_id.to_string());
        doc.observe_update_v1("relay", move |txn, e| {
            let _ = events.send(DocEvent {
                doc_id: id.clone(),
                workspace_id: ws.clone(),
                update: e.update.clone(),
                origin: txn.origin().map(|o| o.as_ref().to_vec()),
            });
        })
        .map_err(|e| store::StoreError::Corrupt(doc_id.into(), e.to_string()))?;
        let live = Arc::new(LiveDoc {
            id: doc_id.to_string(),
            workspace_id: workspace_id.to_string(),
            doc,
            subscribers: Mutex::new(HashMap::new()),
            awareness: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            last_active: Mutex::new(Instant::now()),
        });
        docs.insert(doc_id.to_string(), live.clone());
        Ok(live)
    }

    pub fn loaded(&self, doc_id: &str) -> Option<Arc<LiveDoc>> {
        self.docs.lock().unwrap().get(doc_id).cloned()
    }

    /// Drops all live documents of a deleted workspace; pending writes are discarded.
    pub fn close_workspace(&self, workspace_id: &str) {
        let mut docs = self.docs.lock().unwrap();
        docs.retain(|_, live| {
            if live.workspace_id == workspace_id {
                live.closed.store(true, Ordering::Relaxed);
                false
            } else {
                true
            }
        });
    }

    fn persist(&self, ev: &DocEvent) {
        if let Some(live) = self.loaded(&ev.doc_id)
            && live.closed.load(Ordering::Relaxed)
        {
            return;
        }
        let kind = if ev.doc_id == ev.workspace_id {
            DocKind::Workspace
        } else {
            DocKind::Page
        };
        let count = {
            let conn = self.db.lock();
            store::ensure_doc(&conn, &ev.doc_id, &ev.workspace_id, kind)
                .and_then(|_| store::append_update(&conn, &ev.doc_id, &ev.update))
        };
        match count {
            Ok(n) if n >= store::COMPACT_AFTER => {
                if let Some(live) = self.loaded(&ev.doc_id) {
                    let mut conn = self.db.lock();
                    if let Err(e) = store::compact(&mut conn, &ev.doc_id, &live.doc) {
                        tracing::error!("compact {} failed: {e}", ev.doc_id);
                    }
                }
            }
            Ok(_) => {}
            Err(e) => tracing::error!("persisting update for {} failed: {e}", ev.doc_id),
        }
    }
}

async fn event_loop(reg: Arc<DocRegistry>, mut rx: mpsc::UnboundedReceiver<DocEvent>) {
    while let Some(ev) = rx.recv().await {
        reg.persist(&ev);
        if let Some(live) = reg.loaded(&ev.doc_id) {
            live.touch();
            let except = ev
                .origin
                .as_deref()
                .and_then(|o| std::str::from_utf8(o).ok())
                .and_then(|o| o.strip_prefix("conn:"))
                .and_then(|n| n.parse::<ConnId>().ok());
            let frame = Frame::new(
                Kind::Sync,
                &ev.doc_id,
                protocol::update_message(ev.update.clone()),
            );
            live.broadcast(&frame, except);
        }
        let _ = reg.dirty.send((ev.doc_id, ev.workspace_id));
    }
}

/// Refreshes `page_text` (titles, markdown, deleted flags) and workspace names.
async fn index_loop(reg: Arc<DocRegistry>, mut rx: mpsc::UnboundedReceiver<(String, String)>) {
    loop {
        let Some(first) = rx.recv().await else { return };
        let mut batch: HashSet<(String, String)> = HashSet::from([first]);
        let deadline = tokio::time::sleep(INDEX_DEBOUNCE);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => break,
                next = rx.recv() => match next {
                    Some(item) => { batch.insert(item); }
                    None => break,
                },
            }
        }
        for (doc_id, workspace_id) in batch {
            if let Err(e) = reindex(&reg, &doc_id, &workspace_id) {
                tracing::warn!("reindex {doc_id} failed: {e}");
            }
        }
    }
}

pub fn reindex(reg: &DocRegistry, doc_id: &str, workspace_id: &str) -> Result<(), DocError> {
    let ws = reg.get_or_load(workspace_id, workspace_id, false)?;
    let pages = kawruh_core::workspace::all_pages(&ws.doc);
    if doc_id == workspace_id {
        let name = kawruh_core::workspace::workspace_name(&ws.doc);
        let conn = reg.db.lock();
        if !name.is_empty() {
            db::set_workspace_name(&conn, workspace_id, &name).map_err(store::StoreError::from)?;
        }
        for p in &pages {
            db::upsert_page_text(
                &conn,
                &p.id,
                workspace_id,
                Some(&p.title),
                None,
                Some(p.deleted != 0),
            )
            .map_err(store::StoreError::from)?;
        }
    } else {
        let live = reg.get_or_load(doc_id, workspace_id, false)?;
        let markdown = kawruh_core::markdown::doc_to_markdown(&live.doc);
        let meta = pages.iter().find(|p| p.id == doc_id);
        let conn = reg.db.lock();
        db::upsert_page_text(
            &conn,
            doc_id,
            workspace_id,
            meta.map(|p| p.title.as_str()),
            Some(&markdown),
            meta.map(|p| p.deleted != 0),
        )
        .map_err(store::StoreError::from)?;
    }
    Ok(())
}

async fn janitor(reg: std::sync::Weak<DocRegistry>) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    loop {
        tick.tick().await;
        let Some(reg) = reg.upgrade() else { return };
        let idle: Vec<Arc<LiveDoc>> = {
            let docs = reg.docs.lock().unwrap();
            docs.values()
                .filter(|d| {
                    d.subscriber_count() == 0
                        && d.last_active.lock().unwrap().elapsed() > IDLE_UNLOAD
                })
                .cloned()
                .collect()
        };
        for live in idle {
            {
                let mut conn = reg.db.lock();
                if store::doc_workspace(&conn, &live.id)
                    .ok()
                    .flatten()
                    .is_some()
                    && let Err(e) = store::compact(&mut conn, &live.id, &live.doc)
                {
                    tracing::warn!("compact on unload failed for {}: {e}", live.id);
                    continue;
                }
            }
            let mut docs = reg.docs.lock().unwrap();
            if let Some(cur) = docs.get(&live.id)
                && Arc::ptr_eq(cur, &live)
                && live.subscriber_count() == 0
            {
                docs.remove(&live.id);
            }
        }
    }
}
