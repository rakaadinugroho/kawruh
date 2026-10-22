//! MCP endpoint (`/mcp`, streamable HTTP). Agents read and write pages as
//! markdown; writes go through the live `DocRegistry` (origin "mcp"), so open
//! editors receive them immediately.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

use kawruh_core::comments;
use kawruh_core::database;
use kawruh_core::markdown;
use kawruh_core::workspace;

use crate::auth::{self, AuthUser};
use crate::db::{self, Role};
use crate::docs::{LiveDoc, ORIGIN_MCP};
use crate::error::ApiError;
use crate::share::PAGE_LINK;
use crate::state::AppState;
use crate::{api, assets};

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
    // Requests must fit a base64 image of KAWRUH_MAX_ASSET_MB (upload_image).
    let max_asset = usize::try_from(state.cfg.max_asset_bytes()).unwrap_or(usize::MAX);
    let body_limit = (max_asset / 3)
        .saturating_mul(4)
        .saturating_add(1024 * 1024)
        .max(4 * 1024 * 1024);
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_max_request_body_bytes(body_limit);
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
    /// Initial page body as markdown (GFM). Mermaid diagrams go in ```mermaid fences;
    /// link to another page with [Title](kawruh://page/<page_id>).
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
    /// Link to another page with [Title](kawruh://page/<page_id>).
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
pub struct CommentArgs {
    page_id: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct AddCommentArgs {
    page_id: String,
    /// The comment itself, as markdown. Mention someone with
    /// @[Name](kawruh://user/<user_id>).
    body: String,
    /// The comment this one answers; omit it to start a new thread.
    reply_to: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    page_id: String,
    /// New parent page id; null or omitted moves the page to the top level.
    parent_id: Option<String>,
    /// Position among the new siblings (0 = first); omitted = last.
    index: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct QueryDatabaseArgs {
    page_id: String,
    /// Only rows whose column holds this value. Name the column as it appears
    /// in the database (its name, not its id).
    column: Option<String>,
    value: Option<String>,
    /// How many rows to return at most (50 by default).
    limit: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct UploadImageArgs {
    workspace_id: String,
    /// The image file, base64-encoded (a `data:image/...;base64,` prefix is fine).
    data_base64: String,
    /// Image type: png, jpeg, gif, webp or avif (`image/png` works too).
    mime: String,
    /// Alt text: a short description of the image.
    alt: Option<String>,
    /// Also append the image to the end of this page (in the same workspace).
    page_id: Option<String>,
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

fn decode_image(data: &str) -> Result<Vec<u8>, String> {
    let data = data.split_once("base64,").map_or(data, |(_, d)| d);
    let clean: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(clean)
        .map_err(|e| format!("data_base64 is not valid base64: {e}"))?;
    if bytes.is_empty() {
        return Err("the image is empty".into());
    }
    Ok(bytes)
}

/// Alt text that is safe inside `![...]`: one line, brackets escaped.
fn alt_text(alt: &str) -> String {
    alt.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Whether `markdown` links to `page_id` (`kawruh://page/<page_id>`, not a
/// longer id that merely starts with it).
fn links_to(markdown: &str, page_id: &str) -> bool {
    let needle = format!("{PAGE_LINK}{page_id}");
    markdown.match_indices(&needle).any(|(i, _)| {
        !markdown[i + needle.len()..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
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
        description = "Read a page: title, path and its body as markdown (GFM; ```mermaid fences are diagrams, ```kawruh-canvas fences are drawings, [Title](kawruh://page/<page_id>) links to another page)."
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
        // A database reads as a table, not as its JSON block.
        let body = if meta.kind == database::KIND_DATABASE {
            match database::to_markdown(&live.doc) {
                Some(table) if body.trim().is_empty() => table,
                Some(table) => format!("{body}\n\n{table}"),
                None => body,
            }
        } else {
            body
        };
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
        description = "Create a page (optionally under a parent page) with an optional markdown body. Link to other pages with [Title](kawruh://page/<page_id>). Returns the new page id."
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
        let mut is_database = false;
        if !md.trim().is_empty() {
            // A page that carries a `kawruh-database` block becomes a database.
            match database::split_block(&md) {
                Some((rest, db)) => {
                    workspace::set_page_kind(&ws.doc, &id, database::KIND_DATABASE, ORIGIN_MCP)
                        .map_err(|e| e.to_string())?;
                    database::write_database(&live.doc, &db, ORIGIN_MCP);
                    if !rest.trim().is_empty() {
                        markdown::set_markdown(&live.doc, &rest, ORIGIN_MCP);
                    }
                    is_database = true;
                }
                None => markdown::set_markdown(&live.doc, &md, ORIGIN_MCP),
            }
        }
        {
            let conn = self.state.db.lock();
            let text = database::page_markdown(&live.doc, is_database);
            db::upsert_page_text(
                &conn,
                &id,
                &args.workspace_id,
                Some(args.title.trim()),
                Some(&text),
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
        description = "Replace (default) or append to a page's markdown body. Link to other pages with [Title](kawruh://page/<page_id>). Edits appear live in open editors."
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
        // A body that carries a `kawruh-database` block writes a database.
        match database::split_block(&args.markdown) {
            Some((rest, db)) => {
                let doc = self.workspace_doc(&ws)?;
                workspace::set_page_kind(
                    &doc.doc,
                    &args.page_id,
                    database::KIND_DATABASE,
                    ORIGIN_MCP,
                )
                .map_err(|e| e.to_string())?;
                database::write_database(&live.doc, &db, ORIGIN_MCP);
                match mode {
                    UpdateMode::Replace => {
                        if !rest.trim().is_empty() {
                            markdown::set_markdown(&live.doc, &rest, ORIGIN_MCP);
                        }
                    }
                    UpdateMode::Append => markdown::append_markdown(&live.doc, &rest, ORIGIN_MCP),
                }
            }
            None => match mode {
                UpdateMode::Replace => {
                    markdown::set_markdown(&live.doc, &args.markdown, ORIGIN_MCP)
                }
                UpdateMode::Append => {
                    markdown::append_markdown(&live.doc, &args.markdown, ORIGIN_MCP)
                }
            },
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

    #[tool(
        description = "Upload an image (base64) to a workspace. Returns the markdown that shows it, ![alt](kawruh-asset://<workspace_id>/<name>), to put in a page body; with page_id the image is also appended to that page."
    )]
    async fn upload_image(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<UploadImageArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        self.require_editor(&args.workspace_id, &user)?;
        if let Some(page_id) = &args.page_id {
            let (ws, _) = self.locate(page_id, &user)?;
            if ws != args.workspace_id {
                return Err(format!(
                    "page {page_id} is not in workspace {}",
                    args.workspace_id
                ));
            }
            self.page_meta(&ws, page_id)?;
        }
        let ext = assets::ext_for_mime(&args.mime).ok_or_else(|| {
            format!(
                "unsupported image type \"{}\"; use png, jpeg, gif, webp or avif",
                args.mime
            )
        })?;
        let bytes = decode_image(&args.data_base64)?;
        let (name, _) = assets::store(&self.state, &args.workspace_id, ext, &bytes)
            .await
            .map_err(|e| e.message)?;
        let md = format!(
            "![{}]({}{}/{name})",
            alt_text(args.alt.as_deref().unwrap_or("")),
            assets::SCHEME,
            args.workspace_id
        );
        let Some(page_id) = &args.page_id else {
            return Ok(md);
        };
        let live = self
            .state
            .docs
            .get_or_load(page_id, &args.workspace_id, true)
            .map_err(|e| e.to_string())?;
        markdown::append_markdown(&live.doc, &md, ORIGIN_MCP);
        Ok(format!("{md}\n\nAppended to page {page_id}."))
    }

    #[tool(
        description = "List the pages that link to a page (with [Title](kawruh://page/<page_id>) in their body) as an outline: `- Title (page_id)`. Recent edits are indexed within a few seconds."
    )]
    async fn list_backlinks(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<PageArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        let (meta, all) = self.page_meta(&ws, &args.page_id)?;
        let candidates = {
            let conn = self.state.db.lock();
            db::pages_containing(&conn, &ws, &format!("{PAGE_LINK}{}", args.page_id))
                .map_err(|e| e.to_string())?
        };
        let outline: String = workspace::live(all)
            .iter()
            .filter(|p| p.id != args.page_id)
            .filter(|p| {
                candidates
                    .iter()
                    .any(|(id, md)| *id == p.id && links_to(md, &args.page_id))
            })
            .map(|p| format!("- {} ({})\n", title_or_untitled(&p.title), p.id))
            .collect();
        let target = format!("{} ({})", title_or_untitled(&meta.title), meta.id);
        if outline.is_empty() {
            return Ok(format!("No pages link to {target}."));
        }
        Ok(format!("Pages linking to {target}:\n\n{outline}"))
    }

    #[tool(
        description = "Create (or return the existing) public read-only link to a page and its sub-pages; anyone with the link can read them. Returns the URL."
    )]
    async fn share_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<PageArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, role) = self.locate(&args.page_id, &user)?;
        if !role.can_edit() {
            return Err("you have read-only access to this workspace".into());
        }
        let share = api::ensure_share(&self.state, &ws, &args.page_id, &user.user)
            .map_err(|e| e.message)?;
        Ok(self.state.share_url(&share.token))
    }

    #[tool(
        description = "List the comments on a page as threads: the comment, who wrote it and when, and the replies under it."
    )]
    async fn list_comments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CommentArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        let live = self
            .state
            .docs
            .get_or_load(&args.page_id, &ws, false)
            .map_err(|e| e.to_string())?;
        let threads = comments::threads(&live.doc);
        if threads.is_empty() {
            return Ok(format!("No comments on page {}.", args.page_id));
        }
        let mut out = format!("Comments on page {}:\n", args.page_id);
        for thread in threads {
            let root = &thread[0];
            if !root.quote.is_empty() {
                out.push_str(&format!("> {}\n", root.quote.replace('\n', " ")));
            }
            for c in &thread {
                let who = if c.author_name.is_empty() {
                    "someone"
                } else {
                    &c.author_name
                };
                let marker = if c.id == root.id { "" } else { "  reply: " };
                out.push_str(&format!("{marker}- {who}: {}\n", c.body.replace('\n', " ")));
            }
            out.push('\n');
        }
        Ok(out.trim_end().to_string())
    }

    #[tool(
        description = "Write a comment on a page, or a reply in an existing thread. Mention someone with @[Name](kawruh://user/<user_id>); they get a notification. The comment appears live in open editors."
    )]
    async fn add_comment(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<AddCommentArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        self.require_editor(&ws, &user)?;
        let body = args.body.trim().to_string();
        if body.is_empty() {
            return Err("the comment is empty".into());
        }
        let live = self
            .state
            .docs
            .get_or_load(&args.page_id, &ws, true)
            .map_err(|e| e.to_string())?;
        let now = workspace::now_ms();
        let id = workspace::new_id();
        let comment = match args.reply_to.as_deref().filter(|r| !r.is_empty()) {
            Some(reply_to) => {
                let all = comments::read(&live.doc);
                let parent = all
                    .iter()
                    .find(|c| c.id == reply_to)
                    .ok_or_else(|| format!("comment {reply_to} is not on this page"))?;
                comments::Comment::reply(
                    &id,
                    &parent.thread,
                    &parent.id,
                    &body,
                    &user.user.id,
                    &user.user.display_name,
                    now,
                )
            }
            None => comments::Comment::new(&id, &body, &user.user.id, &user.user.display_name, now),
        };
        comments::write(&live.doc, &comment);
        // Notify the people the comment is about right away; the index loop
        // would do it a moment later.
        {
            let conn = self.state.db.lock();
            crate::notify::sync_page_notifications(&conn, &ws, &args.page_id, &live.doc)
                .map_err(|e| e.to_string())?;
        }
        Ok(format!(
            "Commented on page {}.\nComment ID: {id}",
            args.page_id
        ))
    }

    #[tool(
        description = "Read the rows of a database page as a markdown table. With column and value, only the rows whose column holds that value are returned."
    )]
    async fn query_database(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<QueryDatabaseArgs>,
    ) -> ToolResult {
        let user = caller(&parts)?;
        let (ws, _) = self.locate(&args.page_id, &user)?;
        let live = self
            .state
            .docs
            .get_or_load(&args.page_id, &ws, false)
            .map_err(|e| e.to_string())?;
        let Some(mut db) = database::database(&live.doc) else {
            return Err(format!("page {} is not a database", args.page_id));
        };
        let total = db.rows.len();
        if let Some(column) = args.column.as_deref().filter(|c| !c.is_empty()) {
            let col = db
                .columns
                .iter()
                .find(|c| c.id == column || c.name.eq_ignore_ascii_case(column))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "no column named \"{column}\"; the database has: {}",
                        db.columns
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?;
            let wanted = args.value.clone().unwrap_or_default();
            db.rows.retain(|row| {
                let value = row.cells.get(&col.id).cloned().unwrap_or_default();
                database::json_to_text(&value) == wanted
            });
        }
        let limit = args.limit.unwrap_or(50) as usize;
        db.rows.truncate(limit);
        let table = database::markdown_of(&db);
        Ok(format!(
            "Database on page {} ({} of {total} rows):\n\n{table}",
            args.page_id,
            db.rows.len()
        ))
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
                 are GitHub-flavoured markdown; titles are separate (rename_page). Pages link to each \
                 other with [Title](kawruh://page/<page_id>). Edits are live for everyone viewing the page.",
            )
    }
}
