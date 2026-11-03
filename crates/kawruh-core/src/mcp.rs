//! The shared surface behind our MCP endpoints. The tool layer in
//! `kawruh-mcp` speaks markdown and page ids; where the data comes from —
//! `kawruh-server`'s SQLite and live docs, or the desktop app's local store —
//! is a `McpBackend` implementation. Every call carries the caller so the
//! server can show each agent only the workspaces its token may see; the
//! desktop's single local caller just hands itself in.

use yrs::Doc;

/// Who is on the other end of a tool call (agent's user, device context).
#[derive(Clone)]
pub struct McpCaller {
    pub id: String,
    pub name: String,
}

/// A workspace the caller can see, and whether tools may change it.
pub struct McpWorkspace {
    pub id: String,
    pub name: String,
    pub may_write: bool,
}

/// One search result.
pub struct McpHit {
    pub workspace_id: String,
    pub page_id: String,
    pub title: String,
    pub snippet: String,
}

/// A page found by looking "what links to X" in the caller's workspace.
pub struct McpPageRef {
    pub page_id: String,
    pub title: String,
}

/// Everything `kawruh-mcp`'s tools need, above markdown/page-id level.
/// Sync methods: each starts from a live doc minor work (doc loads, SQLite
/// reads, small file writes) under the host's own locks.
pub trait McpBackend: Send + Sync {
    /// The workspaces the caller may reach (may_write tells whether the
    /// backend accepts page writes in each; e.g. viewer-only or a full disk
    /// turn it off).
    fn workspaces(&self, caller: &McpCaller) -> Result<Vec<McpWorkspace>, String>;

    /// Errors when the caller may not write the workspace, with the reason.
    fn require_write(&self, caller: &McpCaller, workspace_id: &str) -> Result<(), String>;

    /// The live Y.Doc of a page or of the workspace root. `writable` marks a
    /// call that may create the doc (pages elsewhere are read paths).
    fn doc(
        &self,
        caller: &McpCaller,
        doc_id: &str,
        workspace_id: &str,
        writable: bool,
    ) -> Result<Doc, String>;

    /// Which workspace carries `page_id`'s document, when indexed (None is
    /// fine: the tool layer then looks through the caller's trees).
    fn page_workspace(&self, page_id: &str) -> Result<Option<String>, String>;

    /// Page titles and bodies containing every term of the query.
    fn search(
        &self,
        caller: &McpCaller,
        workspace_ids: &[String],
        query: &str,
        limit: usize,
    ) -> Result<Vec<McpHit>, String>;

    /// Live pages of `workspace_id` that link to `page_id`.
    fn backlinks(
        &self,
        caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<Vec<McpPageRef>, String>;

    /// Stores an image; returns its asset *name* (`<hash>.<ext>`), which the
    /// tool layer turns into the ![alt](kawruh-asset://…) markdown.
    fn store_asset(
        &self,
        caller: &McpCaller,
        workspace_id: &str,
        mime: &str,
        bytes: &[u8],
    ) -> Result<String, String>;

    /// Enriches whatever page-text index the host keeps outside the live docs
    /// (newly created pages, mostly). Hosts whose indexes read the live docs
    /// do nothing.
    fn index_page(
        &self,
        caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
        title: &str,
        markdown_text: &str,
        is_database: bool,
    ) -> Result<(), String>;

    /// A comment landed on the page (write its notifications and the like).
    fn comment_written(
        &self,
        _caller: &McpCaller,
        _workspace_id: &str,
        _page_id: &str,
    ) -> Result<(), String> {
        Ok(())
    }

    /// A public read-only URL of the page; only a connected server can hand
    /// them out, so most hosts answer with a clear error instead.
    fn share_page(
        &self,
        caller: &McpCaller,
        workspace_id: &str,
        page_id: &str,
    ) -> Result<String, String>;
}
