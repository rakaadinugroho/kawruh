//! MCP tool set ("Kawruh" for agents), shared by the desktop app and the
//! kawruh-server. Tools read and write pages as markdown; writes go through
//! the host's `McpBackend` (see kawruh_core::mcp), reaching the same live
//! documents open editors see.

use std::sync::Arc;

use base64::Engine;
use http::request::Parts;
use kawruh_core::database;
use kawruh_core::markdown;
use kawruh_core::mcp::{McpCaller, McpWorkspace};
use kawruh_core::workspace;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ServerHandler, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;

type ToolResult = Result<String, String>;

/// The transaction origin every write through here carries, so editors can
/// tell MCP edits apart.
pub const ORIGIN_MCP: &str = "mcp";

/// How a page links to another page, and where stored images live.
const PAGE_LINK: &str = "kawruh://page/";
const ASSET_SCHEME: &str = "kawruh-asset://";

/// Image types `store_asset` may receive.
const IMAGE_TYPES: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
];

/// `mime` is the image type the caller already validated; backends that
/// store per extension reuse this.
pub fn ext_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.trim().to_ascii_lowercase();
    let short = mime.strip_prefix("image/").unwrap_or(&mime);
    let short = if short == "jpeg" { "jpg" } else { short };
    IMAGE_TYPES
        .iter()
        .find(|(e, _)| *e == short)
        .map(|(e, _)| *e)
}

/// The `Service` for one backend: mount it behind your own bearer-auth
/// middleware (whose request extension must carry the `McpCaller`).
///
/// Requests must fit a base64 image of `max_asset_bytes` (upload_image).
pub fn service(
    backend: Arc<dyn kawruh_core::mcp::McpBackend>,
    max_asset_bytes: u64,
) -> StreamableHttpService<KawruhMcp, LocalSessionManager> {
    let body_limit = (max_asset_bytes / 3)
        .saturating_mul(4)
        .saturating_add(1024 * 1024)
        .max(4 * 1024 * 1024);
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .with_max_request_body_bytes(body_limit as usize);
    StreamableHttpService::new(
        move || Ok(KawruhMcp::new(backend.clone())),
        Default::default(),
        config,
    )
}

#[derive(Clone)]
pub struct KawruhMcp {
    backend: Arc<dyn kawruh_core::mcp::McpBackend>,
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

fn caller(parts: &Parts) -> Result<McpCaller, String> {
    parts
        .extensions
        .get::<McpCaller>()
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
/// longer id that merely starts with it). Shared: backends find which pages
/// contain a link the same way.
pub fn links_to(markdown: &str, page_id: &str) -> bool {
    let needle = format!("{PAGE_LINK}{page_id}");
    markdown.match_indices(&needle).any(|(i, _)| {
        !markdown[i + needle.len()..]
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

impl KawruhMcp {
    pub fn new(backend: Arc<dyn kawruh_core::mcp::McpBackend>) -> KawruhMcp {
        KawruhMcp {
            backend,
            tool_router: Self::tool_router(),
        }
    }

    /// Workspace of a page the caller can see, via its index or its tree.
    fn locate(&self, caller: &McpCaller, page_id: &str) -> Result<String, String> {
        let workspaces = self.backend.workspaces(caller)?;
        if let Some(ws) = self.backend.page_workspace(page_id)?
            && workspaces.iter().any(|w| w.id == ws)
        {
            return Ok(ws);
        }
        // Not indexed yet: look through the caller's workspace trees.
        for w in &workspaces {
            let doc = self.backend.doc(caller, &w.id, &w.id, false)?;
            if workspace::all_pages(&doc).iter().any(|p| p.id == page_id) {
                return Ok(w.id.clone());
            }
        }
        Err(format!("page {page_id} not found or not accessible"))
    }

    fn page_meta(
        &self,
        caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<(workspace::PageMeta, Vec<workspace::PageMeta>), String> {
        let ws = self
            .backend
            .doc(caller, workspace_id, workspace_id, false)?;
        let all = workspace::all_pages(&ws);
        let meta = workspace::live(all.clone())
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
        let caller = caller(&parts)?;
        let rows: Vec<_> = self
            .backend
            .workspaces(&caller)?
            .into_iter()
            .map(|w| {
                serde_json::json!({
                    "id": w.id,
                    "name": if w.name.trim().is_empty() { "Untitled" } else { &w.name },
                    "role": if w.may_write { "editor" } else { "viewer" },
                })
            })
            .collect();
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
        let caller = caller(&parts)?;
        let ws_doc = self
            .backend
            .doc(&caller, &args.workspace_id, &args.workspace_id, false)?;
        let pages = workspace::live_pages(&ws_doc);
        let name = workspace::workspace_name(&ws_doc);
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
        description = "Read a page: title, path and its body as markdown (GFM; ```mermaid fences are diagrams, ```kawruh-canvas fences are drawings, [Title](kawruh://page/<page_id>) links to another page). A database page reads as a table of its rows."
    )]
    async fn read_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<PageArgs>,
    ) -> ToolResult {
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        let (meta, all) = self.page_meta(&caller, &ws, &args.page_id)?;
        let doc = self
            .backend
            .doc(&caller, &args.page_id, &ws, false)
            .map_err(|e| e.to_string())?;
        let body = markdown::doc_to_markdown(&doc);
        // A database reads as a table, not as its JSON block.
        let body = if meta.kind == database::KIND_DATABASE {
            match database::to_markdown(&doc) {
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
        let caller = caller(&parts)?;
        let workspaces: Vec<McpWorkspace> = self.backend.workspaces(&caller)?;
        let ids = match &args.workspace_id {
            Some(w) => {
                if !workspaces.iter().any(|x| &x.id == w) {
                    return Err(format!("workspace {w} not found or not accessible"));
                }
                vec![w.clone()]
            }
            None => workspaces.iter().map(|w| w.id.clone()).collect(),
        };
        let hits = self
            .backend
            .search(&caller, &ids, &args.query, 20)
            .map_err(|e| e.to_string())?;
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
        description = "Create a page (optionally under a parent page) with an optional markdown body. Link to other pages with [Title](kawruh://page/<page_id>). A body with a ```kawruh-database fence makes a database page. Returns the new page id."
    )]
    async fn create_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CreateArgs>,
    ) -> ToolResult {
        let caller = caller(&parts)?;
        self.backend.require_write(&caller, &args.workspace_id)?;
        let ws_doc = self
            .backend
            .doc(&caller, &args.workspace_id, &args.workspace_id, true)?;
        let parent = args.parent_id.clone().unwrap_or_default();
        let id = workspace::create_page(&ws_doc, args.title.trim(), &parent, ORIGIN_MCP)
            .map_err(|e| e.to_string())?;
        let page_doc = self
            .backend
            .doc(&caller, &id, &args.workspace_id, true)
            .map_err(|e| e.to_string())?;
        let md = args.markdown.unwrap_or_default();
        let mut is_database = false;
        if !md.trim().is_empty() {
            // A page that carries a `kawruh-database` block becomes a database.
            match database::split_block(&md) {
                Some((rest, db)) => {
                    workspace::set_page_kind(&ws_doc, &id, database::KIND_DATABASE, ORIGIN_MCP)
                        .map_err(|e| e.to_string())?;
                    database::write_database(&page_doc, &db, ORIGIN_MCP);
                    if !rest.trim().is_empty() {
                        markdown::set_markdown(&page_doc, &rest, ORIGIN_MCP);
                    }
                    is_database = true;
                }
                None => markdown::set_markdown(&page_doc, &md, ORIGIN_MCP),
            }
        }
        let text = database::page_markdown(&page_doc, is_database);
        self.backend
            .index_page(
                &caller,
                &args.workspace_id,
                &id,
                args.title.trim(),
                &text,
                is_database,
            )
            .map_err(|e| e.to_string())?;
        Ok(format!(
            "Created page \"{}\" ({id}).",
            title_or_untitled(args.title.trim())
        ))
    }

    #[tool(
        description = "Replace (default) or append to a page's markdown body. Link to other pages with [Title](kawruh://page/<page_id>). Edits appear live in open editors. A body with a ```kawruh-database fence writes a database page."
    )]
    async fn update_page(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<UpdateArgs>,
    ) -> ToolResult {
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        self.backend.require_write(&caller, &ws)?;
        self.page_meta(&caller, &ws, &args.page_id)?;
        let page_doc = self
            .backend
            .doc(&caller, &args.page_id, &ws, true)
            .map_err(|e| e.to_string())?;
        let mode = args.mode.unwrap_or_default();
        // A body that carries a `kawruh-database` block writes a database.
        match database::split_block(&args.markdown) {
            Some((rest, db)) => {
                let ws_doc = self
                    .backend
                    .doc(&caller, &ws, &ws, true)
                    .map_err(|e| e.to_string())?;
                workspace::set_page_kind(
                    &ws_doc,
                    &args.page_id,
                    database::KIND_DATABASE,
                    ORIGIN_MCP,
                )
                .map_err(|e| e.to_string())?;
                database::write_database(&page_doc, &db, ORIGIN_MCP);
                match mode {
                    UpdateMode::Replace => {
                        if !rest.trim().is_empty() {
                            markdown::set_markdown(&page_doc, &rest, ORIGIN_MCP);
                        }
                    }
                    UpdateMode::Append => markdown::append_markdown(&page_doc, &rest, ORIGIN_MCP),
                }
            }
            None => match mode {
                UpdateMode::Replace => {
                    markdown::set_markdown(&page_doc, &args.markdown, ORIGIN_MCP)
                }
                UpdateMode::Append => {
                    markdown::append_markdown(&page_doc, &args.markdown, ORIGIN_MCP)
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        self.backend.require_write(&caller, &ws)?;
        let ws_doc = self
            .backend
            .doc(&caller, &ws, &ws, true)
            .map_err(|e| e.to_string())?;
        workspace::rename_page(&ws_doc, &args.page_id, args.title.trim(), ORIGIN_MCP)
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        self.backend.require_write(&caller, &ws)?;
        let ws_doc = self
            .backend
            .doc(&caller, &ws, &ws, true)
            .map_err(|e| e.to_string())?;
        let parent = args.parent_id.clone().unwrap_or_default();
        workspace::move_page(
            &ws_doc,
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        self.backend.require_write(&caller, &ws)?;
        let ws_doc = self
            .backend
            .doc(&caller, &ws, &ws, true)
            .map_err(|e| e.to_string())?;
        workspace::delete_page(&ws_doc, &args.page_id, ORIGIN_MCP).map_err(|e| e.to_string())?;
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
        let caller = caller(&parts)?;
        self.backend.require_write(&caller, &args.workspace_id)?;
        if let Some(page_id) = &args.page_id {
            let ws = self.locate(&caller, page_id)?;
            if ws != args.workspace_id {
                return Err(format!(
                    "page {page_id} is not in workspace {}",
                    args.workspace_id
                ));
            }
            self.page_meta(&caller, &ws, page_id)?;
        }
        let bytes = decode_image(&args.data_base64)?;
        if ext_for_mime(&args.mime).is_none() {
            return Err(format!(
                "unsupported image type \"{}\"; use png, jpeg, gif, webp or avif",
                args.mime
            ));
        }
        let name = self
            .backend
            .store_asset(&caller, &args.workspace_id, &args.mime, &bytes)
            .map_err(|e| e.to_string())?;
        let md = format!(
            "![{}]({}{}/{name})",
            alt_text(args.alt.as_deref().unwrap_or("")),
            ASSET_SCHEME,
            args.workspace_id
        );
        let Some(page_id) = &args.page_id else {
            return Ok(md);
        };
        let page_doc = self
            .backend
            .doc(&caller, page_id, &args.workspace_id, true)
            .map_err(|e| e.to_string())?;
        markdown::append_markdown(&page_doc, &md, ORIGIN_MCP);
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        let (meta, _) = self.page_meta(&caller, &ws, &args.page_id)?;
        let refs = self
            .backend
            .backlinks(&caller, &ws, &args.page_id)
            .map_err(|e| e.to_string())?;
        let outline = refs
            .iter()
            .filter(|r| r.page_id != args.page_id)
            .map(|r| format!("- {} ({})\n", title_or_untitled(&r.title), r.page_id))
            .collect::<String>();
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        self.page_meta(&caller, &ws, &args.page_id)?;
        self.backend
            .share_page(&caller, &ws, &args.page_id)
            .map_err(|e| e.to_string())
    }

    #[tool(
        description = "List the comments on a page as threads: the comment, who wrote it and when, and the replies under it. Every comment carries its id, which is what add_comment replies to."
    )]
    async fn list_comments(
        &self,
        Extension(parts): Extension<Parts>,
        Parameters(args): Parameters<CommentArgs>,
    ) -> ToolResult {
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        let page_doc = self
            .backend
            .doc(&caller, &args.page_id, &ws, false)
            .map_err(|e| e.to_string())?;
        let threads = kawruh_core::comments::threads(&page_doc);
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
                // The id is what `add_comment` takes to answer this comment.
                out.push_str(&format!(
                    "{marker}- [{}] {who}: {}\n",
                    c.id,
                    c.body.replace('\n', " ")
                ));
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        self.backend.require_write(&caller, &ws)?;
        let body = args.body.trim().to_string();
        if body.is_empty() {
            return Err("the comment is empty".into());
        }
        let page_doc = self
            .backend
            .doc(&caller, &args.page_id, &ws, true)
            .map_err(|e| e.to_string())?;
        let now = workspace::now_ms();
        let id = workspace::new_id();
        let comment = match args.reply_to.as_deref().filter(|r| !r.is_empty()) {
            Some(reply_to) => {
                let all = kawruh_core::comments::read(&page_doc);
                let parent = all
                    .iter()
                    .find(|c| c.id == reply_to)
                    .ok_or_else(|| format!("comment {reply_to} is not on this page"))?;
                kawruh_core::comments::Comment::reply(
                    &id,
                    &parent.thread,
                    &parent.id,
                    &body,
                    &caller.id,
                    &caller.name,
                    now,
                )
            }
            None => kawruh_core::comments::Comment::new(&id, &body, &caller.id, &caller.name, now),
        };
        kawruh_core::comments::write(&page_doc, &comment);
        self.backend
            .comment_written(&caller, &ws, &args.page_id)
            .map_err(|e| e.to_string())?;
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
        let caller = caller(&parts)?;
        let ws = self.locate(&caller, &args.page_id)?;
        let page_doc = self
            .backend
            .doc(&caller, &args.page_id, &ws, false)
            .map_err(|e| e.to_string())?;
        let Some(mut db) = database::database(&page_doc) else {
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
                 other with [Title](kawruh://page/<page_id>). Edits are live for everyone viewing the page. \
                 A page can be a database: put a ```kawruh-database block with its columns and rows in \
                 the body, and read the rows back with query_database.",
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{alt_text, decode_image, ext_for_mime, links_to, service};
    use base64::Engine;

    #[test]
    fn image_types_map_to_extensions() {
        assert_eq!(ext_for_mime("image/png"), Some("png"));
        assert_eq!(ext_for_mime("JPEG"), Some("jpg"));
        assert_eq!(ext_for_mime("image/webp"), Some("webp"));
        assert_eq!(ext_for_mime("application/pdf"), None);
    }

    #[test]
    fn decode_image_takes_the_data_url_form() {
        let data = base64::engine::general_purpose::STANDARD.encode("hello");
        assert_eq!(decode_image(&data).unwrap(), b"hello");
        let data_url = format!("data:image/png;base64,{data}");
        assert_eq!(decode_image(&data_url).unwrap(), b"hello");
        assert!(decode_image("!!!").is_err());
        assert!(decode_image("").is_err());
    }

    #[test]
    fn alt_text_flattens_and_escapes() {
        assert_eq!(alt_text(" line  one\n two "), "line one two");
        assert_eq!(alt_text("a[b]"), "a\\[b\\]");
    }

    #[test]
    fn links_to_needs_the_exact_page_id() {
        let md = "[A](kawruh://page/p1) and [B](kawruh://page/p11)";
        assert!(links_to(md, "p1"));
        assert!(links_to(md, "p11"));
        assert!(!links_to("[B](kawruh://page/p11)", "p1"));
    }

    #[test]
    fn the_body_limit_grows_with_the_asset_cap() {
        // One megabyte of base64 per megabyte of image, plus a small margin:
        // only checks that a stub backend wires up as a service.
        let service = service(std::sync::Arc::new(NullBackend), 6 * 1024 * 1024);
        let _ = std::mem::forget(service);
    }

    struct NullBackend;
    impl kawruh_core::mcp::McpBackend for NullBackend {
        fn workspaces(
            &self,
            _: &kawruh_core::mcp::McpCaller,
        ) -> Result<Vec<kawruh_core::mcp::McpWorkspace>, String> {
            Ok(vec![])
        }
        fn require_write(&self, _: &kawruh_core::mcp::McpCaller, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn doc(
            &self,
            _: &kawruh_core::mcp::McpCaller,
            _: &str,
            _: &str,
            _: bool,
        ) -> Result<yrs::Doc, String> {
            Ok(yrs::Doc::new())
        }
        fn page_workspace(&self, _: &str) -> Result<Option<String>, String> {
            Ok(None)
        }
        fn search(
            &self,
            _: &kawruh_core::mcp::McpCaller,
            _: &[String],
            _: &str,
            _: usize,
        ) -> Result<Vec<kawruh_core::mcp::McpHit>, String> {
            Ok(vec![])
        }
        fn backlinks(
            &self,
            _: &kawruh_core::mcp::McpCaller,
            _: &str,
            _: &str,
        ) -> Result<Vec<kawruh_core::mcp::McpPageRef>, String> {
            Ok(vec![])
        }
        fn store_asset(
            &self,
            _: &kawruh_core::mcp::McpCaller,
            _: &str,
            _: &str,
            _: &[u8],
        ) -> Result<String, String> {
            Ok("x.png".into())
        }
        fn index_page(
            &self,
            _: &kawruh_core::mcp::McpCaller,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            _: bool,
        ) -> Result<(), String> {
            Ok(())
        }
        fn share_page(
            &self,
            _: &kawruh_core::mcp::McpCaller,
            _: &str,
            _: &str,
        ) -> Result<String, String> {
            Err("no sharing here".into())
        }
    }
}
