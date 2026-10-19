//! Client side of the workspace sync socket (used by the desktop app and tests).
//!
//! Keeps one WebSocket per workspace alive with exponential backoff. Every
//! (re)connect re-subscribes all documents and exchanges state vectors, so
//! edits made while offline converge without an explicit queue.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use yrs::Doc;

use crate::protocol::{self, Frame, Kind, SyncAction};

/// Transaction origin for updates received from the server.
pub const ORIGIN_REMOTE: &str = "remote";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SyncStatus {
    Connecting,
    Online,
    Offline,
    Unauthorized,
}

/// What the sync client needs from its owner.
pub trait SyncHost: Send + Sync + 'static {
    /// The document to sync; loaded or created on demand.
    fn doc(&self, doc_id: &str) -> Option<Doc>;
    fn on_status(&self, status: SyncStatus);
    fn on_awareness(&self, doc_id: &str, update: Vec<u8>);
    fn on_error(&self, doc_id: &str, code: &str, message: &str);
}

enum Cmd {
    Subscribe(String),
    Unsubscribe(String),
    Update(String, Vec<u8>),
    Awareness(String, Vec<u8>),
    Shutdown,
}

#[derive(Clone)]
pub struct SyncHandle {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl SyncHandle {
    pub fn subscribe(&self, doc_id: &str) {
        let _ = self.tx.send(Cmd::Subscribe(doc_id.to_string()));
    }
    pub fn unsubscribe(&self, doc_id: &str) {
        let _ = self.tx.send(Cmd::Unsubscribe(doc_id.to_string()));
    }
    /// Sends a local update. Dropped while offline; the next reconnect's
    /// state-vector exchange delivers it instead.
    pub fn send_update(&self, doc_id: &str, update: Vec<u8>) {
        let _ = self.tx.send(Cmd::Update(doc_id.to_string(), update));
    }
    pub fn send_awareness(&self, doc_id: &str, update: Vec<u8>) {
        let _ = self.tx.send(Cmd::Awareness(doc_id.to_string(), update));
    }
    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

/// `ws://host/api/ws?workspace=<id>` for an `http(s)://host` server URL.
pub fn ws_url(server: &str, workspace_id: &str) -> String {
    let base = server.trim_end_matches('/');
    let base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{base}/api/ws?workspace={workspace_id}")
}

pub fn spawn(url: String, token: String, host: Arc<dyn SyncHost>) -> SyncHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(url, token, host, rx));
    SyncHandle { tx }
}

enum Exit {
    Shutdown,
    Disconnected,
}

async fn run(
    url: String,
    token: String,
    host: Arc<dyn SyncHost>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
) {
    let mut subscribed: BTreeSet<String> = BTreeSet::new();
    let mut backoff = Duration::from_millis(500);
    install_tls_provider();
    loop {
        host.on_status(SyncStatus::Connecting);
        let mut request = match url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("invalid sync url {url}: {e}");
                host.on_status(SyncStatus::Offline);
                return;
            }
        };
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {token}")) {
            request.headers_mut().insert("Authorization", v);
        }
        // Keep listening for commands while connecting, so a shutdown isn't
        // held up by a slow or hanging handshake.
        let connect = tokio_tungstenite::connect_async(request);
        tokio::pin!(connect);
        let result = loop {
            tokio::select! {
                res = &mut connect => break res,
                cmd = rx.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => return,
                    Some(Cmd::Subscribe(id)) => { subscribed.insert(id); }
                    Some(Cmd::Unsubscribe(id)) => { subscribed.remove(&id); }
                    Some(Cmd::Update(..)) | Some(Cmd::Awareness(..)) => {}
                },
            }
        };
        match result {
            Ok((ws, _)) => {
                backoff = Duration::from_millis(500);
                host.on_status(SyncStatus::Online);
                match session(ws, &host, &mut rx, &mut subscribed).await {
                    Exit::Shutdown => {
                        host.on_status(SyncStatus::Offline);
                        return;
                    }
                    Exit::Disconnected => host.on_status(SyncStatus::Offline),
                }
            }
            Err(e) => {
                let unauthorized = matches!(
                    &e,
                    tokio_tungstenite::tungstenite::Error::Http(r)
                        if r.status() == 401 || r.status() == 403
                );
                tracing::debug!("sync connect failed: {e}");
                host.on_status(if unauthorized {
                    SyncStatus::Unauthorized
                } else {
                    SyncStatus::Offline
                });
                if unauthorized {
                    backoff = Duration::from_secs(30);
                }
            }
        }
        // Wait out the backoff, still tracking subscriptions.
        let sleep = tokio::time::sleep(backoff);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                cmd = rx.recv() => match cmd {
                    None | Some(Cmd::Shutdown) => return,
                    Some(Cmd::Subscribe(id)) => { subscribed.insert(id); }
                    Some(Cmd::Unsubscribe(id)) => { subscribed.remove(&id); }
                    Some(Cmd::Update(..)) | Some(Cmd::Awareness(..)) => {}
                },
            }
        }
        backoff = (backoff * 2).min(Duration::from_secs(8));
    }
}

async fn session<S>(
    ws: tokio_tungstenite::WebSocketStream<S>,
    host: &Arc<dyn SyncHost>,
    rx: &mut mpsc::UnboundedReceiver<Cmd>,
    subscribed: &mut BTreeSet<String>,
) -> Exit
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let (mut sink, mut stream) = ws.split();

    async fn send<K>(sink: &mut K, frame: Frame) -> bool
    where
        K: futures_util::Sink<Message> + Unpin,
    {
        sink.send(Message::Binary(frame.encode().into()))
            .await
            .is_ok()
    }

    let subscribe_frames = |id: &str| -> Option<[Frame; 2]> {
        let doc = host.doc(id)?;
        Some([
            Frame::new(Kind::Subscribe, id, Vec::new()),
            Frame::new(Kind::Sync, id, protocol::step1(&doc)),
        ])
    };

    for id in subscribed.clone() {
        for f in subscribe_frames(&id).into_iter().flatten() {
            if !send(&mut sink, f).await {
                return Exit::Disconnected;
            }
        }
    }

    let mut ping = tokio::time::interval(Duration::from_secs(15));
    ping.tick().await;
    // A path that dies silently (laptop leaves the LAN) never closes the
    // socket; the server answers pings, so silence means it's gone.
    let mut last_rx = tokio::time::Instant::now();
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let ok = match cmd {
                    None | Some(Cmd::Shutdown) => {
                        let _ = sink.send(Message::Close(None)).await;
                        return Exit::Shutdown;
                    }
                    Some(Cmd::Subscribe(id)) => {
                        if subscribed.insert(id.clone()) {
                            let mut ok = true;
                            for f in subscribe_frames(&id).into_iter().flatten() {
                                ok &= send(&mut sink, f).await;
                            }
                            ok
                        } else {
                            true
                        }
                    }
                    Some(Cmd::Unsubscribe(id)) => {
                        subscribed.remove(&id);
                        send(&mut sink, Frame::new(Kind::Unsubscribe, &id, Vec::new())).await
                    }
                    Some(Cmd::Update(id, u)) => {
                        if subscribed.contains(&id) {
                            send(&mut sink, Frame::new(Kind::Sync, &id, protocol::update_message(u))).await
                        } else {
                            true
                        }
                    }
                    Some(Cmd::Awareness(id, u)) => {
                        send(&mut sink, Frame::new(Kind::Awareness, &id, u)).await
                    }
                };
                if !ok {
                    return Exit::Disconnected;
                }
            }
            msg = stream.next() => {
                last_rx = tokio::time::Instant::now();
                let bytes = match msg {
                    Some(Ok(Message::Binary(b))) => b,
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Exit::Disconnected,
                    Some(Ok(_)) => continue,
                };
                let Ok(frame) = Frame::decode(&bytes) else { continue };
                match frame.kind {
                    Kind::Sync => {
                        let Some(doc) = host.doc(&frame.doc_id) else { continue };
                        match protocol::handle_sync(&doc, &frame.payload, ORIGIN_REMOTE) {
                            Ok(SyncAction::Reply(reply)) => {
                                if !send(&mut sink, Frame::new(Kind::Sync, &frame.doc_id, reply)).await {
                                    return Exit::Disconnected;
                                }
                            }
                            Ok(SyncAction::Applied) => {}
                            Err(e) => tracing::warn!("sync error on {}: {e}", frame.doc_id),
                        }
                    }
                    Kind::Awareness => host.on_awareness(&frame.doc_id, frame.payload),
                    Kind::Error => {
                        let v: serde_json::Value = serde_json::from_slice(&frame.payload).unwrap_or_default();
                        host.on_error(
                            &frame.doc_id,
                            v["code"].as_str().unwrap_or("error"),
                            v["message"].as_str().unwrap_or(""),
                        );
                    }
                    Kind::Subscribe | Kind::Unsubscribe => {}
                }
            }
            _ = ping.tick() => {
                if last_rx.elapsed() > Duration::from_secs(40) {
                    return Exit::Disconnected;
                }
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Exit::Disconnected;
                }
            }
        }
    }
}

/// wss:// needs a rustls crypto provider; pick ring explicitly rather than rely
/// on whichever one other crates happen to enable.
fn install_tls_provider() {
    #[cfg(feature = "tls")]
    let _ = rustls::crypto::ring::default_provider().install_default();
}

#[cfg(all(test, feature = "tls"))]
mod tests {
    use tokio_tungstenite::tungstenite::Error;
    use tokio_tungstenite::tungstenite::error::UrlError;

    /// Public servers sit behind HTTPS, so the client must speak wss://.
    /// Against a socket that hangs up, a TLS-capable client fails in the
    /// handshake, not with "TLS support not compiled in".
    #[tokio::test]
    async fn wss_urls_are_supported() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = listener.accept().await;
        });
        super::install_tls_provider();
        let err = tokio_tungstenite::connect_async(format!("wss://localhost:{port}/"))
            .await
            .unwrap_err();
        assert!(
            !matches!(err, Error::Url(UrlError::TlsFeatureNotEnabled)),
            "{err:?}"
        );
    }
}
