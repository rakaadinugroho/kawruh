//! MCP endpoint (`/mcp`, streamable HTTP). Agents read and write pages as
//! markdown; writes go through the live `DocRegistry` (origin "mcp"), so open
//! editors receive them immediately.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use kawruh_core::markdown;
use kawruh_core::workspace;

use crate::auth::{self, AuthUser};
use crate::db::{self, Role};
use crate::docs::{LiveDoc, ORIGIN_MCP};
use crate::error::ApiError;
use crate::state::AppState;

type Parts = http::request::Parts;
type ToolResult = Result<String, String>;

/// Rejects `/mcp` requests without a valid bearer token and attaches the user.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let (mut parts, body) = req.into_parts();
    let Some(token) = auth::bearer(&parts) else {
        return ApiError::unauthorized().into_response();
    };
    match auth::authenticate(&state.db, &token) {
        Ok(user) => {
            parts.extensions.insert(user);
            next.run(Request::from_parts(parts, body)).await
        }
        Err(e) => e.into_response(),
    }
}

pub fn service(state: AppState) -> StreamableHttpService<KawruhMcp, LocalSessionManager> {
    // Bearer auth is enforced in front of the service, so the Host allow-list
    // (DNS-rebinding protection for unauthenticated local servers) is not needed.
    let config = StreamableHttpServerConfig::default().disable_allowed_hosts();
    StreamableHttpService::new(
        move || Ok(KawruhMcp::new(state.clone())),
        Default::default(),
        config,
    )
}

#[derive(Clone)]
pub struct KawruhMcp {
    state: AppState,
    tool_router: ToolRouter<Self>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct WorkspaceArgs {
    /// Workspace id (from list_workspaces).
    workspace_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct PageArgs {
    /// Page id (from list_pages or search_pages).
    page_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Text to look for in page titles and content (case-insensitive).
    query: String,
    /// Limit the search to one workspace.
    workspace_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateArgs {
    workspace_id: String,
    title: String,
    /// Initial page body as markdown (GFM). Mermaid diagrams go in ```mermaid fences.
    markdown: Option<String>,
    /// Parent page id; omit to create a top-level page.
    parent_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    #[default]
    Replace,
    Append,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct UpdateArgs {
    page_id: String,
    /// New page body as markdown (without the title; use rename_page for that).
    markdown: String,
    /// "replace" (default) overwrites the body, "append" adds to the end.
    mode: Option<UpdateMode>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct RenameArgs {
    page_id: String,
    title: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    page_id: String,
    /// New parent page id; null or omitted moves the page to the top level.
    parent_id: Option<String>,
    /// Position among the new siblings (0 = first); omitted = last.
    index: Option<u32>,
}

fn caller(parts: &Parts) -> Result<AuthUser, String> {
    parts
        .extensions
        .get::<AuthUser>()
        .cloned()
        .ok_or_else(|| "unauthorized".to_string())
}

fn title_or_untitled(t: &str) -> &str {
    if t.trim().is_empty() { "Untitled" } else { t }
}

impl KawruhMcp {
    pub fn new(state: AppState) -> KawruhMcp {
        KawruhMcp {
            state,
            tool_router: Self::tool_router(),
        }
    }

    fn role(&self, workspace_id: &str, user: &AuthUser) -> Result<Role, String> {
        let conn = self.state.db.lock();
        db::role_of(&conn, workspace_id, &user.user.id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("workspace {workspace_id} not found or not accessible"))
    }

    fn require_editor(&self, workspace_id: &str, user: &AuthUser) -> Result<(), String> {
        if !self.role(workspace_id, user)?.can_edit() {
            return Err("you have read-only access to this workspace".into());
        }
        if !self.state.docs.writes_allowed() {
            return Err("the server is low on disk space; writes are refused".into());
        }
        Ok(())
    }

    fn workspace_doc(&self, workspace_id: &str) -> Result<std::sync::Arc<LiveDoc>, String> {
        self.state
            .docs
            .get_or_load(workspace_id, workspace_id, true)
            .map_err(|e| e.to_string())
    }

    /// Workspace of a page the caller can access, plus the caller's role.
    fn locate(&self, page_id: &str, user: &AuthUser) -> Result<(String, Role), String> {
        let known = {
            let conn = self.state.db.lock();
            match db::page_workspace(&conn, page_id).map_err(|e| e.to_string())? {
                Some(ws) => Some(ws),
                None => {
                    kawruh_core::store::doc_workspace(&conn, page_id).map_err(|e| e.to_string())?
                }
            }
        };
        if let Some(ws) = known
            && let Ok(role) = self.role(&ws, user)
        {
            return Ok((ws, role));
        }
        // Not indexed yet: look through the caller's workspace trees.
        let workspaces = {
            let conn = self.state.db.lock();
            db::workspaces_of(&conn, &user.user.id).map_err(|e| e.to_string())?
        };
        for w in workspaces {
            let doc = self.workspace_doc(&w.id)?;
            if workspace::all_pages(&doc.doc)
                .iter()
                .any(|p| p.id == page_id)
            {
                return Ok((w.id, w.role));
            }
        }
        Err(format!("page {page_id} not found or not accessible"))
    }

    fn page_meta(
        &self,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<(workspace::PageMeta, Vec<workspace::PageMeta>), String> {
        let ws = self.workspace_doc(workspace_id)?;
        let all = workspace::all_pages(&ws.doc);
        let live = workspace::live(all.clone());
        let meta = live
            .iter()
            .find(|p| p.id == page_id)
            .cloned()
            .ok_or_else(|| format!("page {page_id} is deleted or not in this workspace"))?;
        Ok((meta, all))
    }
}

#[tool_router]
impl KawruhMcp {
    #[tool(description = "List the Kawruh workspaces you can access, with your role in each.")]
    async fn list_workspaces(&self, Extension(parts): Extension<Parts>) -> ToolResult {
        let user = caller(&parts)?;
        let conn = self.state.db.lock();
        let rows = db::workspaces_of(&conn, &user.user.id).map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())
    }

    #[tool(
        description = "List the pages of a workspace as an indented outline: `- Title (page_id)`."
    )]
    async fn list_pages(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<WorkspaceArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        self.role(&args.workspace_id, &user)?;
        let ws = self.workspace_doc(&args.workspace_id)?;
        let pages = workspace::live_pages(&ws.doc);
        let name = workspace::workspace_name(&ws.doc);
        let outline = workspace::outline(&pages);
        Ok(format!(
            "Workspace: {} ({})\n\n{}",
            title_or_untitled(&name),
            args.workspace_id,
            if outline.is_empty() {
                "(no pages)\n".to_string()
            } else {
                outline
            }
        ))
    }

    #[tool(
        description = "Read a page: title, path and its body as markdown (GFM; ```mermaid fences are diagrams, ```kawruh-canvas fences are drawings)."
    )]
    async fn read_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<PageArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        let (meta, all) = self.page_meta(&ws, &args.page_id)?;
        let live = self
            .state
            .docs
            .get_or_load(&args.page_id, &ws, false)
            .map_err(|e| e.to_string())?;
        let body = markdown::doc_to_markdown(&live.doc);
        let path = workspace::path(&all, &args.page_id)
            .iter()
            .map(|t| title_or_untitled(t).to_string())
            .collect::<Vec<_>>()
            .join(" / ");
        Ok(format!(
            "Title: {}\nPath: {}\nPage ID: {}\nWorkspace ID: {}\n\n---\n\n{}",
            title_or_untitled(&meta.title),
            path,
            meta.id,
            ws,
            body
        ))
    }

    #[tool(
        description = "Search page titles and content (case-insensitive substring). Recently edited pages are indexed within a few seconds."
    )]
    async fn search_pages(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<SearchArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let conn = self.state.db.lock();
        let workspaces = db::workspaces_of(&conn, &user.user.id).map_err(|e| e.to_string())?;
        let ids: Vec<String> = match &args.workspace_id {
            Some(w) => {
                if !workspaces.iter().any(|x| &x.id == w) {
                    return Err(format!("workspace {w} not found or not accessible"));
                }
                vec![w.clone()]
            }
            None => workspaces.iter().map(|w| w.id.clone()).collect(),
        };
        let hits = db::search(&conn, &ids, &args.query, 20).map_err(|e| e.to_string())?;
        if hits.is_empty() {
            return Ok(format!("No pages match \"{}\".", args.query));
        }
        let name_of = |id: &str| {
            workspaces
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.name.clone())
                .unwrap_or_default()
        };
        Ok(hits
            .iter()
            .map(|h| {
                format!(
                    "- {} ({}) in {}: {}",
                    title_or_untitled(&h.title),
                    h.page_id,
                    name_of(&h.workspace_id),
                    h.snippet.trim()
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[tool(
        description = "Create a page (optionally under a parent page) with an optional markdown body. Returns the new page id."
    )]
    async fn create_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CreateArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        self.require_editor(&args.workspace_id, &user)?;
        let ws = self.workspace_doc(&args.workspace_id)?;
        let parent = args.parent_id.clone().unwrap_or_default();
        let id = workspace::create_page(&ws.doc, args.title.trim(), &parent, ORIGIN_MCP)
            .map_err(|e| e.to_string())?;
        let live = self
            .state
            .docs
            .get_or_load(&id, &args.workspace_id, true)
            .map_err(|e| e.to_string())?;
        let md = args.markdown.unwrap_or_default();
        if !md.trim().is_empty() {
            markdown::set_markdown(&live.doc, &md, ORIGIN_MCP);
        }
        {
            let conn = self.state.db.lock();
            db::upsert_page_text(
                &conn,
                &id,
                &args.workspace_id,
                Some(args.title.trim()),
                Some(&md),
                Some(false),
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(format!(
            "Created page \"{}\" ({id}).",
            title_or_untitled(args.title.trim())
        ))
    }

    #[tool(
        description = "Replace (default) or append to a page's markdown body. Edits appear live in open editors."
    )]
    async fn update_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<UpdateArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        self.require_editor(&ws, &user)?;
        self.page_meta(&ws, &args.page_id)?;
        let live = self
            .state
            .docs
            .get_or_load(&args.page_id, &ws, true)
            .map_err(|e| e.to_string())?;
        let mode = args.mode.unwrap_or_default();
        match mode {
            UpdateMode::Replace => markdown::set_markdown(&live.doc, &args.markdown, ORIGIN_MCP),
            UpdateMode::Append => markdown::append_markdown(&live.doc, &args.markdown, ORIGIN_MCP),
        }
        Ok(format!(
            "{} page {}.",
            if mode == UpdateMode::Append {
                "Appended to"
            } else {
                "Updated"
            },
            args.page_id
        ))
    }

    #[tool(description = "Rename a page.")]
    async fn rename_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<RenameArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        self.require_editor(&ws, &user)?;
        let doc = self.workspace_doc(&ws)?;
        workspace::rename_page(&doc.doc, &args.page_id, args.title.trim(), ORIGIN_MCP)
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Renamed page {} to \"{}\".",
            args.page_id,
            args.title.trim()
        ))
    }

    #[tool(
        description = "Move a page under another page (or to the top level), optionally at a position among its new siblings."
    )]
    async fn move_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<MoveArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        self.require_editor(&ws, &user)?;
        let doc = self.workspace_doc(&ws)?;
        let parent = args.parent_id.clone().unwrap_or_default();
        workspace::move_page(
            &doc.doc,
            &args.page_id,
            &parent,
            args.index.map(|i| i as usize),
            ORIGIN_MCP,
        )
        .map_err(|e| e.to_string())?;
        Ok(format!(
            "Moved page {} to {}.",
            args.page_id,
            if parent.is_empty() {
                "the top level".to_string()
            } else {
                format!("under {parent}")
            }
        ))
    }

    #[tool(
        description = "Move a page (and its sub-pages) to the trash. It can be restored from the app's Trash."
    )]
    async fn delete_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<PageArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        self.require_editor(&ws, &user)?;
        let doc = self.workspace_doc(&ws)?;
        workspace::delete_page(&doc.doc, &args.page_id, ORIGIN_MCP).map_err(|e| e.to_string())?;
        Ok(format!("Moved page {} to the trash.", args.page_id))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for KawruhMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("kawruh", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Kawruh is a markdown notebook organised as workspaces containing a tree of pages. \
                 Use list_workspaces, then list_pages or search_pages to find page ids. Page bodies \
                 are GitHub-flavoured markdown; titles are separate (rename_page). Edits are live for \
                 everyone viewing the page.",
            )
    }
}
