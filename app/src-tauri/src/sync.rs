//! Keeps one sync connection per remote workspace and feeds it from the hub.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kawruh_core::sync_client::{self, SyncHandle, SyncHost, SyncStatus};
use kawruh_core::workspace;
use kawruh_core::yrs::Doc;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::hub::DocHub;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusEvent<'a> {
    workspace_id: &'a str,
    status: SyncStatus,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorEvent<'a> {
    workspace_id: &'a str,
    doc_id: &'a str,
    code: &'a str,
    message: &'a str,
}

struct Host {
    workspace_id: String,
    hub: Arc<DocHub>,
    app: AppHandle,
    statuses: Arc<Mutex<HashMap<String, SyncStatus>>>,
    /// Set when this connection is replaced or the workspace is forgotten; a
    /// task still winding down must not touch documents or status any more.
    stopped: Arc<AtomicBool>,
}

impl SyncHost for Host {
    fn doc(&self, doc_id: &str) -> Option<Doc> {
        if self.stopped.load(Ordering::Acquire) {
            return None;
        }
        match self.hub.get_or_load(doc_id, &self.workspace_id) {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!("sync: cannot load {doc_id}: {e}");
                None
            }
        }
    }

    fn on_status(&self, status: SyncStatus) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        self.statuses
            .lock()
            .unwrap()
            .insert(self.workspace_id.clone(), status);
        if matches!(status, SyncStatus::Online) {
            // Images pasted while offline (or before publishing) go up now.
            tauri::async_runtime::spawn(crate::assets::flush(
                self.app.clone(),
                self.workspace_id.clone(),
            ));
        }
        let _ = self.app.emit(
            "sync-status",
            StatusEvent {
                workspace_id: &self.workspace_id,
                status,
            },
        );
    }

    fn on_awareness(&self, doc_id: &str, update: Vec<u8>) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        self.hub.deliver_awareness(doc_id, &update);
    }

    fn on_error(&self, doc_id: &str, code: &str, message: &str) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        tracing::warn!("sync error {code} on {doc_id}: {message}");
        // The server says we can't write: the UI must stop offering edits.
        if code == "readonly" {
            let _ = crate::db::set_workspace_role(
                &self.hub.db.lock().unwrap(),
                &self.workspace_id,
                "viewer",
            );
        }
        let _ = self.app.emit(
            "sync-error",
            ErrorEvent {
                workspace_id: &self.workspace_id,
                doc_id,
                code,
                message,
            },
        );
    }
}

pub struct SyncManager {
    hub: Arc<DocHub>,
    app: AppHandle,
    conns: Mutex<HashMap<String, (SyncHandle, Arc<AtomicBool>)>>,
    statuses: Arc<Mutex<HashMap<String, SyncStatus>>>,
}

impl SyncManager {
    pub fn new(hub: Arc<DocHub>, app: AppHandle) -> Arc<SyncManager> {
        Arc::new(SyncManager {
            hub,
            app,
            conns: Mutex::new(HashMap::new()),
            statuses: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn start(&self, workspace_id: &str, server_url: &str, token: &str) {
        // Commands may run outside the async runtime; the client spawns tasks.
        let _rt = tauri::async_runtime::handle().inner().enter();
        self.stop(workspace_id);
        let stopped = Arc::new(AtomicBool::new(false));
        let host = Arc::new(Host {
            workspace_id: workspace_id.to_string(),
            hub: self.hub.clone(),
            app: self.app.clone(),
            statuses: self.statuses.clone(),
            stopped: stopped.clone(),
        });
        let handle = sync_client::spawn(
            sync_client::ws_url(server_url, workspace_id),
            token.to_string(),
            host,
        );
        self.conns
            .lock()
            .unwrap()
            .insert(workspace_id.to_string(), (handle, stopped));
        self.subscribe_all(workspace_id);
    }

    pub fn stop(&self, workspace_id: &str) {
        if let Some((h, stopped)) = self.conns.lock().unwrap().remove(workspace_id) {
            stopped.store(true, Ordering::Release);
            h.shutdown();
        }
        self.statuses.lock().unwrap().remove(workspace_id);
    }

    pub fn status(&self, workspace_id: &str) -> Option<SyncStatus> {
        self.statuses.lock().unwrap().get(workspace_id).copied()
    }

    /// Subscribes the workspace doc and every page it lists (deleted ones too,
    /// so Trash stays restorable offline). Idempotent.
    pub fn subscribe_all(&self, workspace_id: &str) {
        let Some(handle) = self
            .conns
            .lock()
            .unwrap()
            .get(workspace_id)
            .map(|(h, _)| h.clone())
        else {
            return;
        };
        handle.subscribe(workspace_id);
        if let Ok(doc) = self.hub.get_or_load(workspace_id, workspace_id) {
            for p in workspace::all_pages(&doc) {
                handle.subscribe(&p.id);
            }
        }
    }

    pub fn send_update(&self, workspace_id: &str, doc_id: &str, update: Vec<u8>) {
        if let Some((h, _)) = self.conns.lock().unwrap().get(workspace_id) {
            h.send_update(doc_id, update);
        }
    }

    pub fn send_awareness(&self, workspace_id: &str, doc_id: &str, update: Vec<u8>) {
        if let Some((h, _)) = self.conns.lock().unwrap().get(workspace_id) {
            h.send_awareness(doc_id, update);
        }
    }
}
