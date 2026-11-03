//! The desktop app's own MCP endpoint for agents running on this computer:
//! one axum listener on a localhost port, protected by a bearer token, backed
//! by the local store (see `crate::mcp`). The Settings toggle owns whether it
//! runs, and the toggle's state — including the token — is what agents get
//! registered with.

use std::sync::{Arc, Mutex};

use axum::extract::{DefaultBodyLimit, Request};
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use kawruh_core::mcp::McpBackend;
use serde::Serialize;
use tauri::State;

use crate::commands::AppState;
use crate::db;
use crate::error::Result;
use crate::hub::DocHub;
use crate::mcp::{DesktopBackend, local_caller};
use crate::search::SearchIndex;

pub const DEFAULT_PORT: u16 = 7421;
/// The Settings toggle (`mcpLocal`: "true"/"false") and the bearer token
/// agents authenticate with (generated once; regenerate only by hand).
pub const SETTING_ENABLED: &str = "mcpLocal";
pub const SETTING_TOKEN: &str = "mcp_local_token";

/// BASE64 of a 10 MB image eats ~13.7 MB, so the request ceiling sits above.
const BODY_LIMIT: usize = 24 * 1024 * 1024;

struct Running {
    port: u16,
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// Owns the endpoint lifecycle: at most one listener, started from the
/// Settings toggle or at launch when the stored setting says so.
pub struct McpRuntime {
    hub: Arc<DocHub>,
    search: Arc<SearchIndex>,
    running: Mutex<Option<Running>>,
}

impl McpRuntime {
    pub fn new(hub: Arc<DocHub>, search: Arc<SearchIndex>) -> McpRuntime {
        McpRuntime {
            hub,
            search,
            running: Mutex::new(None),
        }
    }

    /// The port of the running endpoint, if one is up right now.
    pub fn port(&self) -> Option<u16> {
        self.running.lock().unwrap().as_ref().map(|r| r.port)
    }

    pub(crate) fn enabled_setting(&self) -> bool {
        db::get_setting(&self.hub.db.lock().unwrap(), SETTING_ENABLED)
            .ok()
            .flatten()
            .as_deref()
            == Some("true")
    }

    /// The stored token, or a freshly generated one (kept for next time).
    pub(crate) fn token(&self) -> Result<String> {
        let conn = self.hub.db.lock().unwrap();
        if let Some(t) = db::get_setting(&conn, SETTING_TOKEN)?.filter(|t| !t.is_empty()) {
            return Ok(t);
        }
        let token = uuid::Uuid::new_v4().simple().to_string();
        db::set_setting(&conn, SETTING_TOKEN, &token)?;
        Ok(token)
    }

    /// Starts the listener unless it already runs; returns its port.
    pub async fn start(&self) -> Result<u16> {
        if let Some(port) = self.port() {
            return Ok(port);
        }
        let port = port_configured();
        let token = self.token()?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| {
                crate::error::Error::msg(match e.kind() {
                    std::io::ErrorKind::AddrInUse => {
                        format!("Port {port} is already in use (is another Kawruh running?)")
                    }
                    _ => format!("Cannot listen on port {port}: {e}"),
                })
            })?;
        let backend: Arc<dyn McpBackend> = Arc::new(DesktopBackend {
            hub: Arc::clone(&self.hub),
            search: Arc::clone(&self.search),
        });
        let (shutdown, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tauri::async_runtime::spawn(async move {
            let server =
                axum::serve(listener, router(backend, token)).with_graceful_shutdown(async move {
                    let _ = stopped.await;
                });
            if let Err(e) = server.await {
                tracing::warn!("mcp endpoint stopped early: {e}");
            }
        });
        *self.running.lock().unwrap() = Some(Running {
            port,
            shutdown,
            task,
        });
        tracing::info!("mcp endpoint on http://127.0.0.1:{port}/mcp");
        Ok(port)
    }

    /// Stops the listener (a no-op when nothing runs). The accept loop closes
    /// at once; an agent's open SSE session is left to finish on its own
    /// rather than holding the toggle hostage.
    pub async fn stop(&self) -> Result<()> {
        let Some(r) = self.running.lock().unwrap().take() else {
            return Ok(());
        };
        let _ = r.shutdown.send(());
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), r.task).await;
        tracing::info!("mcp endpoint stopped");
        Ok(())
    }

    fn state(&self, running_port: Option<u16>) -> Result<McpLocalState> {
        let port = running_port.unwrap_or_else(port_configured);
        Ok(McpLocalState {
            enabled: self.enabled_setting(),
            running: running_port.is_some(),
            port,
            url: format!("http://127.0.0.1:{port}/mcp"),
            token: self.token()?,
        })
    }
}

/// How the endpoint configures itself for `mcp_local_set`'s and startup.
pub fn port_configured() -> u16 {
    std::env::var("KAWRUH_MCP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// What the Settings tab (and agent registration) work with.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct McpLocalState {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub url: String,
    pub token: String,
}

/// `/mcp` behind bearer auth, the caller carrying the local identity.
fn router(backend: Arc<dyn McpBackend>, token: String) -> axum::Router {
    axum::Router::new()
        .nest_service(
            "/mcp",
            kawruh_mcp::service(backend, crate::assets::MAX_BYTES as u64),
        )
        .layer(axum::middleware::from_fn(
            move |req: Request, next: Next| {
                let token = token.clone();
                async move { guard(token, req, next).await }
            },
        ))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

async fn guard(token: String, req: Request, next: Next) -> Response {
    let present = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(format!("Bearer {token}").as_str());
    if !present {
        return (StatusCode::UNAUTHORIZED, "kawruh mcp: bad bearer token").into_response();
    }
    let mut req = req;
    req.extensions_mut().insert(local_caller());
    next.run(req).await
}

// ------------------------------------------------------------ tauri commands

#[tauri::command]
pub async fn mcp_local_state(state: State<'_, AppState>) -> Result<McpLocalState> {
    state.mcp.state(state.mcp.port())
}

/// Toggles the endpoint (the Settings switch); the state it lands on is
/// returned, so the UI keeps its URL/token read straight from the store.
#[tauri::command]
pub async fn mcp_local_set(state: State<'_, AppState>, enabled: bool) -> Result<McpLocalState> {
    {
        let conn = state.hub.db.lock().unwrap();
        db::set_setting(
            &conn,
            SETTING_ENABLED,
            if enabled { "true" } else { "false" },
        )?;
    }
    let running_port = if enabled {
        Some(state.mcp.start().await?)
    } else {
        state.mcp.stop().await?;
        None
    };
    state.mcp.state(running_port)
}
