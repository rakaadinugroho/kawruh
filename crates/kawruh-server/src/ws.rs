//! The workspace sync socket (`GET /api/ws?workspace=<id>`).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use yrs::Update;
use yrs::sync::SyncMessage;
use yrs::updates::decoder::Decode;

use kawruh_core::protocol::{self, Frame, Kind};

use crate::api::{require_role, valid_id};
use crate::auth::AuthUser;
use crate::db::Role;
use crate::docs::{ConnId, DocError, LiveDoc, conn_origin};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct WsQuery {
    workspace: String,
}

pub async fn handler(
    State(state): State<AppState>,
    Query(q): Query<WsQuery>,
    me: AuthUser,
    upgrade: WebSocketUpgrade,
) -> ApiResult<Response> {
    if !valid_id(&q.workspace) {
        return Err(ApiError::bad_request("invalid", "invalid workspace id"));
    }
    let role = require_role(&state, &q.workspace, &me.user, Role::Viewer)?;
    Ok(upgrade
        .max_message_size(32 * 1024 * 1024)
        .on_upgrade(move |socket| connection(state, socket, q.workspace, me.user.id, role)))
}

/// How long a connection trusts its cached role before re-reading it, so
/// demotions and removals take effect on open sockets too.
const ROLE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

struct Conn {
    id: ConnId,
    state: AppState,
    workspace_id: String,
    user_id: String,
    role: Role,
    role_checked: std::time::Instant,
    revoked: bool,
    outbox: mpsc::UnboundedSender<Vec<u8>>,
    subs: HashMap<String, Arc<LiveDoc>>,
    warned_readonly: bool,
}

impl Conn {
    fn send(&self, frame: Frame) {
        let _ = self.outbox.send(frame.encode());
    }

    fn error(&self, doc_id: &str, code: &str, message: &str) {
        self.send(Frame::error(doc_id, code, message));
    }

    /// Re-reads the role when the cached one is stale. Returns false (and
    /// marks the connection for closing) once the user is no longer a member.
    fn refresh_role(&mut self) -> bool {
        if self.role_checked.elapsed() < ROLE_TTL {
            return true;
        }
        self.role_checked = std::time::Instant::now();
        let role = crate::db::role_of(&self.state.db.lock(), &self.workspace_id, &self.user_id);
        match role {
            Ok(Some(role)) => {
                if role != self.role {
                    self.warned_readonly = false;
                }
                self.role = role;
                true
            }
            Ok(None) => {
                self.error(
                    "",
                    "forbidden",
                    "you are no longer a member of this workspace",
                );
                self.revoked = true;
                false
            }
            Err(e) => {
                tracing::error!("role lookup failed: {e}");
                true
            }
        }
    }

    fn handle(&mut self, frame: Frame) {
        match frame.kind {
            Kind::Subscribe => self.subscribe(&frame.doc_id),
            Kind::Unsubscribe => self.unsubscribe(&frame.doc_id),
            Kind::Sync => self.sync(&frame.doc_id, &frame.payload),
            Kind::Awareness => {
                if let Some(live) = self.subs.get(&frame.doc_id)
                    && live.apply_awareness(self.id, &frame.payload)
                {
                    live.broadcast(&frame, Some(self.id));
                }
            }
            Kind::Error => {}
        }
    }

    fn subscribe(&mut self, doc_id: &str) {
        if !valid_id(doc_id) {
            return self.error(doc_id, "invalid", "invalid document id");
        }
        if self.subs.contains_key(doc_id) {
            return;
        }
        let live =
            match self
                .state
                .docs
                .get_or_load(doc_id, &self.workspace_id, self.role.can_edit())
            {
                Ok(live) => live,
                Err(DocError::Forbidden) => {
                    return self.error(
                        doc_id,
                        "forbidden",
                        "document belongs to another workspace",
                    );
                }
                Err(e) => {
                    tracing::error!("loading {doc_id}: {e}");
                    return self.error(doc_id, "internal", "could not load document");
                }
            };
        live.add_subscriber(self.id, self.outbox.clone());
        self.send(Frame::new(Kind::Sync, doc_id, protocol::step1(&live.doc)));
        if let Some(aw) = live.awareness_snapshot() {
            self.send(Frame::new(Kind::Awareness, doc_id, aw));
        }
        self.subs.insert(doc_id.to_string(), live);
    }

    fn unsubscribe(&mut self, doc_id: &str) {
        if let Some(live) = self.subs.remove(doc_id)
            && let Some(removal) = live.remove_subscriber(self.id)
        {
            live.broadcast(&Frame::new(Kind::Awareness, doc_id, removal), Some(self.id));
        }
    }

    fn sync(&mut self, doc_id: &str, payload: &[u8]) {
        let Some(live) = self.subs.get(doc_id).cloned() else {
            return self.error(doc_id, "not_subscribed", "subscribe before syncing");
        };
        let msg = match protocol::decode_sync(payload) {
            Ok(m) => m,
            Err(e) => return self.error(doc_id, "bad_message", &e.to_string()),
        };
        match msg {
            SyncMessage::SyncStep1(sv) => {
                self.send(Frame::new(
                    Kind::Sync,
                    doc_id,
                    protocol::answer_step1(&live.doc, &sv),
                ));
            }
            SyncMessage::SyncStep2(u) | SyncMessage::Update(u) => {
                // A viewer's step2 is normally empty; only real changes are refused.
                let empty = Update::decode_v1(&u).map(|u| u.is_empty()).unwrap_or(false);
                if empty {
                    return;
                }
                if !self.refresh_role() {
                    return;
                }
                if !self.role.can_edit() {
                    if !self.warned_readonly {
                        self.warned_readonly = true;
                        self.error(
                            doc_id,
                            "readonly",
                            "you have read-only access to this workspace",
                        );
                    }
                    return;
                }
                if !self.state.docs.writes_allowed() {
                    return self.error(
                        doc_id,
                        "disk_full",
                        "server is low on disk space; change not saved",
                    );
                }
                if let Err(e) = protocol::apply_update(&live.doc, &u, &conn_origin(self.id)) {
                    self.error(doc_id, "bad_update", &e.to_string());
                }
            }
        }
    }

    fn close(&mut self) {
        let ids: Vec<String> = self.subs.keys().cloned().collect();
        for id in ids {
            self.unsubscribe(&id);
        }
    }
}

async fn connection(
    state: AppState,
    socket: WebSocket,
    workspace_id: String,
    user_id: String,
    role: Role,
) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let writer = tokio::spawn(async move {
        while let Some(bytes) = rx.recv().await {
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let mut conn = Conn {
        id: state.next_conn_id(),
        state,
        workspace_id,
        user_id,
        role,
        role_checked: std::time::Instant::now(),
        revoked: false,
        outbox: tx,
        subs: HashMap::new(),
        warned_readonly: false,
    };
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Binary(bytes)) => match Frame::decode(&bytes) {
                Ok(frame) => {
                    conn.handle(frame);
                    if conn.revoked {
                        break;
                    }
                }
                Err(e) => conn.error("", "bad_frame", &e.to_string()),
            },
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    conn.close();
    drop(conn);
    let _ = writer.await;
}
