//! MCP endpoint (`/mcp`, streamable HTTP). The tool layer lives in the
//! `kawruh-mcp` crate; this module is the server's backend for it — each
//! request's token decides which workspaces its caller sees, and writes land
//! in the live `DocRegistry` (origin "mcp") so open editors receive them at
//! once.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use kawruh_core::mcp::{McpBackend, McpCaller, McpHit, McpPageRef, McpWorkspace};
use kawruh_core::workspace;
use rmcp::transport::streamable_http_server::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use yrs::Doc;

use kawruh_mcp::KawruhMcp;

use crate::api;
use crate::assets;
use crate::auth;
use crate::db;
use crate::docs::DocError;
use crate::error::ApiError;
use crate::share::PAGE_LINK;
use crate::state::AppState;

/// Rejects `/mcp` requests without a valid bearer token, naming the caller
/// (`McpCaller` extension) for the tool layer.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let Some(token) = auth::bearer(&parts) else {
        return ApiError::unauthorized().into_response();
    };
    match auth::authenticate(&state.db, &token) {
        Ok(user) => {
            parts.extensions.insert(McpCaller {
                id: user.user.id.clone(),
                name: user.user.display_name.clone(),
            });
            parts.extensions.insert(user);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(e) => e.into_response(),
    }
}

pub fn service(state: AppState) -> StreamableHttpService<KawruhMcp, LocalSessionManager> {
    // Bearer auth is enforced in front of the service, so the Host allow-list
    // (DNS-rebinding protection for unauthenticated local servers) is not needed.
    kawruh_mcp::service(Arc::new(state.clone()), state.cfg.max_asset_bytes())
}

impl McpBackend for AppState {
    fn workspaces(&self, caller: &McpCaller) -> Result<Vec<McpWorkspace>, String> {
        let conn = self.db.lock();
        Ok(db::workspaces_of(&conn, &caller.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|w| McpWorkspace {
                id: w.id,
                name: w.name,
                may_write: w.role.can_edit(),
            })
            .collect())
    }

    fn require_write(&self, caller: &McpCaller, workspace_id: &str) -> Result<(), String> {
        let conn = self.db.lock();
        let role = db::role_of(&conn, workspace_id, &caller.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("workspace {workspace_id} not found or not accessible"))?;
        if !role.can_edit() {
            return Err("you have read-only access to this workspace".into());
        }
        if !self.docs.writes_allowed() {
            return Err("the server is low on disk space; writes are refused".into());
        }
        Ok(())
    }

    fn doc(
        &self,
        _caller: &McpCaller,
        doc_id: &str,
        workspace_id: &str,
        register: bool,
    ) -> Result<Doc, String> {
        self.docs
            .get_or_load(doc_id, workspace_id, register)
            .map(|l| l.doc.clone())
            .map_err(|e| match e {
                DocError::Forbidden => "the document belongs to another workspace".into(),
                DocError::Store(s) => s.to_string(),
            })
    }

    fn page_workspace(&self, page_id: &str) -> Result<Option<String>, String> {
        let conn = self.db.lock();
        match db::page_workspace(&conn, page_id).map_err(|e| e.to_string())? {
            Some(ws) => Ok(Some(ws)),
            None => kawruh_core::store::doc_workspace(&conn, page_id).map_err(|e| e.to_string()),
        }
    }

    fn search(
        &self,
        _caller: &McpCaller,
        workspace_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<McpHit>, String> {
        let conn = self.db.lock();
        Ok(db::search(&conn, workspace_ids, query, limit)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|h| McpHit {
                workspace_id: h.workspace_id,
                page_id: h.page_id,
                title: h.title,
                snippet: h.snippet,
            })
            .collect())
    }

    fn backlinks(
        &self,
        _caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<Vec<McpPageRef>, String> {
        let ws = self
            .docs
            .get_or_load(workspace_id, workspace_id, false)
            .map_err(|e| match e {
                DocError::Forbidden => "the document belongs to another workspace".into(),
                DocError::Store(s) => s.to_string(),
            })?;
        let candidates = {
            let conn = self.db.lock();
            db::pages_containing(&conn, workspace_id, &format!("{PAGE_LINK}{page_id}"))
                .map_err(|e| e.to_string())?
        };
        Ok(workspace::live(workspace::all_pages(&ws.doc))
            .iter()
            .filter(|p| p.id != page_id)
            .filter(|p| {
                candidates
                    .iter()
                    .any(|(id, md)| *id == p.id && kawruh_mcp::links_to(md, page_id))
            })
            .map(|p| McpPageRef {
                page_id: p.id.clone(),
                title: p.title.clone(),
            })
            .collect())
    }

    fn store_asset(
        &self,
        _caller: &McpCaller,
        workspace_id: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<String, String> {
        let ext = kawruh_mcp::ext_for_mime(mime)
            .ok_or_else(|| format!("unsupported image type \"{mime}\""))?;
        // The sync twin of `assets::store`: no runtime bridging, so it works
        // from any tokio flavor the host runs (a small blocking write on the
        // worker is the same trade the desktop's backend makes).
        assets::store_sync(self, workspace_id, ext, bytes)
            .map(|(name, _)| name)
            .map_err(|e| e.message)
    }

    fn index_page(
        &self,
        _caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
        title: &str,
        markdown_text: &str,
        _is_database: bool,
    ) -> Result<(), String> {
        let conn = self.db.lock();
        db::upsert_page_text(
            &conn,
            page_id,
            workspace_id,
            Some(title),
            Some(markdown_text),
            Some(false),
        )
        .map_err(|e| e.to_string())
    }

    fn comment_written(
        &self,
        _caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<(), String> {
        let live = self
            .docs
            .get_or_load(page_id, workspace_id, false)
            .map_err(|e| match e {
                DocError::Forbidden => "the document belongs to another workspace".into(),
                DocError::Store(s) => s.to_string(),
            })?;
        let conn = self.db.lock();
        crate::notify::sync_page_notifications(&conn, workspace_id, page_id, &live.doc)
            .map_err(|e| e.to_string())
    }

    fn share_page(
        &self,
        caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<String, String> {
        self.require_write(caller, workspace_id)?;
        let share =
            api::ensure_share(self, workspace_id, page_id, &caller.id).map_err(|e| e.message)?;
        Ok(self.share_url(&share.token))
    }
}
