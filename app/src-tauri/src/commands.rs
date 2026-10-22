//! Tauri commands: the whole webview ↔ Rust surface (see app/src/lib/ipc.ts).

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use kawruh_core::{markdown, workspace};
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use crate::assets;
use crate::db::{self, Account, WorkspaceRow};
use crate::error::{Error, Result};
use crate::hub::{DocEvent, DocHub, ORIGIN_LOCAL};
use crate::remote::{ApiToken, Member, Remote, RemoteWorkspace, Share, normalize_server};
use crate::sync::SyncManager;
use crate::transfer;

pub struct AppState {
    pub hub: Arc<DocHub>,
    pub sync: Arc<SyncManager>,
    pub search: Arc<crate::search::SearchIndex>,
    pub history: Arc<crate::history::History>,
    pub remote: Remote,
    pub data_dir: std::path::PathBuf,
}

const WELCOME: &str = include_str!("welcome.md");

fn decode(b64: &str) -> Result<Vec<u8>> {
    B64.decode(b64)
        .map_err(|e| Error::msg(format!("bad base64: {e}")))
}

impl AppState {
    fn db(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.hub.db.lock().unwrap()
    }

    fn account(&self, id: &str) -> Result<Account> {
        let a = db::get_account(&self.db(), id)?.ok_or_else(|| Error::msg("account not found"))?;
        if a.token.is_empty() {
            return Err(Error::Remote {
                status: 401,
                code: "logged_out".into(),
                message: format!("Log in to {} again", a.server_url),
            });
        }
        Ok(a)
    }

    /// Account of a remote workspace.
    pub(crate) fn workspace_account(&self, workspace_id: &str) -> Result<Account> {
        let ws = db::get_workspace(&self.db(), workspace_id)?
            .ok_or_else(|| Error::msg("workspace not found"))?;
        let id = ws
            .account_id
            .ok_or_else(|| Error::msg("this workspace is local; publish it to a server first"))?;
        self.account(&id)
    }

    /// Starts syncing every remote workspace whose account is logged in.
    pub fn start_all_syncs(&self) -> Result<()> {
        let rows = db::list_workspaces(&self.db())?;
        for ws in rows {
            if let Some(acc) = ws.account_id.as_deref()
                && let Ok(a) = self.account(acc)
            {
                self.sync.start(&ws.id, &a.server_url, &a.token);
            }
        }
        Ok(())
    }

    fn workspace_info(&self, row: WorkspaceRow) -> WorkspaceInfo {
        let status = if row.account_id.is_some() {
            self.sync
                .status(&row.id)
                .map(|s| serde_json::to_value(s).unwrap_or_default())
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_else(|| "offline".into())
        } else {
            "local".into()
        };
        WorkspaceInfo { row, status }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    #[serde(flatten)]
    row: WorkspaceRow,
    status: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: &'static str,
    data_dir: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        data_dir: state.data_dir.display().to_string(),
    }
}

// ---------------------------------------------------------------- workspaces

#[tauri::command]
pub fn workspaces_list(state: State<'_, AppState>) -> Result<Vec<WorkspaceInfo>> {
    let rows = db::list_workspaces(&state.db())?;
    Ok(rows.into_iter().map(|r| state.workspace_info(r)).collect())
}

/// Seeds a new workspace doc: its name and a welcome page.
fn seed_workspace(state: &AppState, id: &str, name: &str, welcome: bool) -> Result<()> {
    let doc = state.hub.get_or_load(id, id)?;
    workspace::set_workspace_name(&doc, name, ORIGIN_LOCAL);
    if welcome {
        let page = workspace::create_page(&doc, "Welcome to Kawruh", "", ORIGIN_LOCAL)?;
        let page_doc = state.hub.get_or_load(&page, id)?;
        markdown::set_markdown(&page_doc, WELCOME, ORIGIN_LOCAL);
    }
    Ok(())
}

#[tauri::command]
pub async fn workspace_create(
    state: State<'_, AppState>,
    name: String,
    account_id: Option<String>,
) -> Result<WorkspaceInfo> {
    let name = if name.trim().is_empty() {
        "My notes".to_string()
    } else {
        name.trim().to_string()
    };
    let (id, role) = match account_id.as_deref() {
        Some(acc) => {
            let a = state.account(acc)?;
            let ws = state
                .remote
                .create_workspace(&a.server_url, &a.token, None, &name)
                .await?;
            (ws.id, ws.role)
        }
        None => (workspace::new_id(), "owner".to_string()),
    };
    db::insert_workspace(&state.db(), &id, &name, account_id.as_deref(), &role)?;
    seed_workspace(&state, &id, &name, true)?;
    if let Some(acc) = account_id.as_deref() {
        let a = state.account(acc)?;
        state.sync.start(&id, &a.server_url, &a.token);
    }
    let row =
        db::get_workspace(&state.db(), &id)?.ok_or_else(|| Error::msg("workspace vanished"))?;
    Ok(state.workspace_info(row))
}

/// Removes a workspace from this device (the server copy is untouched).
#[tauri::command]
pub fn workspace_forget(state: State<'_, AppState>, workspace_id: String) -> Result<()> {
    state.sync.stop(&workspace_id);
    state.hub.unload_workspace(&workspace_id);
    db::delete_workspace(&mut state.db(), &workspace_id)?;
    Ok(())
}

/// Uploads a local workspace to a server under the same id and starts syncing.
#[tauri::command]
pub async fn workspace_publish(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    account_id: String,
) -> Result<WorkspaceInfo> {
    let a = state.account(&account_id)?;
    let row = db::get_workspace(&state.db(), &workspace_id)?
        .ok_or_else(|| Error::msg("workspace not found"))?;
    let ws = state
        .remote
        .create_workspace(&a.server_url, &a.token, Some(&workspace_id), &row.name)
        .await?;
    db::insert_workspace(
        &state.db(),
        &workspace_id,
        &row.name,
        Some(&account_id),
        &ws.role,
    )?;
    state.sync.start(&workspace_id, &a.server_url, &a.token);
    tauri::async_runtime::spawn(assets::flush(app, workspace_id.clone()));
    let row = db::get_workspace(&state.db(), &workspace_id)?
        .ok_or_else(|| Error::msg("workspace vanished"))?;
    Ok(state.workspace_info(row))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspaceInfo {
    #[serde(flatten)]
    ws: RemoteWorkspace,
    attached: bool,
}

#[tauri::command]
pub async fn remote_workspaces(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<RemoteWorkspaceInfo>> {
    let a = state.account(&account_id)?;
    let list = state.remote.workspaces(&a.server_url, &a.token).await?;
    let db = state.db();
    let mut out = Vec::new();
    for ws in list {
        let local = db::get_workspace(&db, &ws.id)?;
        if local.is_some() {
            db::set_workspace_role(&db, &ws.id, &ws.role)?;
        }
        out.push(RemoteWorkspaceInfo {
            attached: local.is_some(),
            ws,
        });
    }
    Ok(out)
}

/// Adds a server workspace to this device and starts syncing it.
#[tauri::command]
pub async fn workspace_attach(
    state: State<'_, AppState>,
    account_id: String,
    workspace_id: String,
) -> Result<WorkspaceInfo> {
    let a = state.account(&account_id)?;
    let list = state.remote.workspaces(&a.server_url, &a.token).await?;
    let ws = list
        .into_iter()
        .find(|w| w.id == workspace_id)
        .ok_or_else(|| Error::msg("you are not a member of that workspace"))?;
    db::insert_workspace(&state.db(), &ws.id, &ws.name, Some(&account_id), &ws.role)?;
    state.sync.start(&ws.id, &a.server_url, &a.token);
    let row =
        db::get_workspace(&state.db(), &ws.id)?.ok_or_else(|| Error::msg("workspace vanished"))?;
    Ok(state.workspace_info(row))
}

// ------------------------------------------------------------------ accounts

#[tauri::command]
pub fn accounts_list(state: State<'_, AppState>) -> Result<Vec<serde_json::Value>> {
    let accounts = db::list_accounts(&state.db())?;
    Ok(accounts
        .into_iter()
        .map(|a| {
            let logged_in = !a.token.is_empty();
            let mut v = serde_json::to_value(a).unwrap_or_default();
            v["loggedIn"] = serde_json::json!(logged_in);
            v
        })
        .collect())
}

#[tauri::command]
pub async fn account_login(
    state: State<'_, AppState>,
    server_url: String,
    username: String,
    password: String,
    register: bool,
    display_name: Option<String>,
    invite: Option<String>,
) -> Result<Account> {
    let server = normalize_server(&server_url);
    let res = state
        .remote
        .auth(
            &server,
            &username,
            &password,
            register,
            display_name.as_deref(),
            invite.as_deref(),
        )
        .await?;
    let account = db::upsert_account(
        &state.db(),
        &Account {
            id: workspace::new_id(),
            server_url: server,
            user_id: res.user.id,
            username: res.user.username,
            display_name: res.user.display_name,
            token: res.token,
        },
    )?;
    // Resume syncing workspaces that belong to this account.
    state.start_all_syncs()?;
    Ok(account)
}

#[tauri::command]
pub async fn account_logout(state: State<'_, AppState>, account_id: String) -> Result<()> {
    if let Ok(a) = state.account(&account_id) {
        let _ = state.remote.logout(&a.server_url, &a.token).await;
    }
    let rows = db::list_workspaces(&state.db())?;
    for ws in rows
        .iter()
        .filter(|w| w.account_id.as_deref() == Some(&account_id))
    {
        state.sync.stop(&ws.id);
    }
    db::clear_token(&state.db(), &account_id)?;
    Ok(())
}

// ------------------------------------------------------- members and shares

#[tauri::command]
pub async fn members_list(state: State<'_, AppState>, workspace_id: String) -> Result<Vec<Member>> {
    let a = state.workspace_account(&workspace_id)?;
    state
        .remote
        .members(&a.server_url, &a.token, &workspace_id)
        .await
}

#[tauri::command]
pub async fn member_add(
    state: State<'_, AppState>,
    workspace_id: String,
    username: String,
    role: String,
) -> Result<Member> {
    let a = state.workspace_account(&workspace_id)?;
    state
        .remote
        .add_member(
            &a.server_url,
            &a.token,
            &workspace_id,
            username.trim(),
            &role,
        )
        .await
}

#[tauri::command]
pub async fn member_remove(
    state: State<'_, AppState>,
    workspace_id: String,
    user_id: String,
) -> Result<()> {
    let a = state.workspace_account(&workspace_id)?;
    state
        .remote
        .remove_member(&a.server_url, &a.token, &workspace_id, &user_id)
        .await
}

#[tauri::command]
pub async fn shares_list(state: State<'_, AppState>, workspace_id: String) -> Result<Vec<Share>> {
    let a = state.workspace_account(&workspace_id)?;
    state
        .remote
        .shares(&a.server_url, &a.token, &workspace_id)
        .await
}

#[tauri::command]
pub async fn share_create(
    state: State<'_, AppState>,
    workspace_id: String,
    page_id: String,
) -> Result<Share> {
    let a = state.workspace_account(&workspace_id)?;
    let res = state
        .remote
        .create_share(&a.server_url, &a.token, &workspace_id, &page_id)
        .await;
    // The server only knows pages that have synced; say so instead of "not found".
    match res {
        Err(Error::Remote { status, code, .. }) if code == "page_not_found" => Err(Error::Remote {
            status,
            code,
            message: "This page hasn't reached the server yet. Wait until the workspace shows \
                      Synced, then try again."
                .into(),
        }),
        other => other,
    }
}

#[tauri::command]
pub async fn share_delete(
    state: State<'_, AppState>,
    workspace_id: String,
    token: String,
) -> Result<()> {
    let a = state.workspace_account(&workspace_id)?;
    state
        .remote
        .delete_share(&a.server_url, &a.token, &token)
        .await
}

#[tauri::command]
pub async fn tokens_list(state: State<'_, AppState>, account_id: String) -> Result<Vec<ApiToken>> {
    let a = state.account(&account_id)?;
    state.remote.tokens(&a.server_url, &a.token).await
}

/// An invite code for this account's server, for someone to sign up with.
#[tauri::command]
pub async fn invite_create(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<crate::remote::Invite> {
    let a = state.account(&account_id)?;
    state.remote.create_invite(&a.server_url, &a.token).await
}

#[tauri::command]
pub async fn token_create(
    state: State<'_, AppState>,
    account_id: String,
    name: String,
) -> Result<ApiToken> {
    let a = state.account(&account_id)?;
    state
        .remote
        .create_token(&a.server_url, &a.token, &name)
        .await
}

#[tauri::command]
pub async fn token_delete(
    state: State<'_, AppState>,
    account_id: String,
    token_id: String,
) -> Result<()> {
    let a = state.account(&account_id)?;
    state
        .remote
        .delete_token(&a.server_url, &a.token, &token_id)
        .await
}

// ----------------------------------------------------------------- documents

#[derive(Serialize)]
pub struct Opened {
    view: u32,
    state: String,
}

#[tauri::command]
pub fn doc_open(
    state: State<'_, AppState>,
    doc_id: String,
    workspace_id: String,
    channel: Channel<DocEvent>,
) -> Result<Opened> {
    let (view, bytes) = state.hub.open_view(&doc_id, &workspace_id, channel)?;
    // A page created on this device must start syncing right away.
    state.sync.subscribe_all(&workspace_id);
    if doc_id != workspace_id {
        // How the page looked when opened, before this session's edits.
        crate::snapshot_if_due(&state.hub, &state.history, &workspace_id, &doc_id);
    }
    Ok(Opened {
        view,
        state: B64.encode(bytes),
    })
}

#[tauri::command]
pub fn doc_close(state: State<'_, AppState>, doc_id: String, view: u32) {
    state.hub.close_view(&doc_id, view);
}

#[tauri::command]
pub fn doc_apply(
    state: State<'_, AppState>,
    doc_id: String,
    view: u32,
    update: String,
) -> Result<()> {
    state.hub.apply_from_view(&doc_id, view, &decode(&update)?)
}

#[tauri::command]
pub fn awareness_send(
    state: State<'_, AppState>,
    workspace_id: String,
    doc_id: String,
    update: String,
) -> Result<()> {
    state
        .sync
        .send_awareness(&workspace_id, &doc_id, decode(&update)?);
    Ok(())
}

#[tauri::command]
pub fn page_export_markdown(
    state: State<'_, AppState>,
    workspace_id: String,
    page_id: String,
) -> Result<String> {
    let ws = state.hub.get_or_load(&workspace_id, &workspace_id)?;
    let title = workspace::all_pages(&ws)
        .into_iter()
        .find(|p| p.id == page_id)
        .map(|p| p.title)
        .unwrap_or_default();
    let doc = state.hub.get_or_load(&page_id, &workspace_id)?;
    let body = markdown::doc_to_markdown(&doc);
    Ok(if title.is_empty() {
        body
    } else {
        format!("# {title}\n\n{body}")
    })
}

/// Creates a page from markdown. A leading `# Title` becomes the page title.
#[tauri::command]
pub fn page_import_markdown(
    state: State<'_, AppState>,
    workspace_id: String,
    parent: String,
    title: String,
    markdown_text: String,
) -> Result<String> {
    import_markdown(&state, &workspace_id, &parent, &title, &markdown_text)
}

fn import_markdown(
    state: &AppState,
    workspace_id: &str,
    parent: &str,
    title: &str,
    markdown_text: &str,
) -> Result<String> {
    let (title, body) = split_title(title, markdown_text);
    let ws = state.hub.get_or_load(workspace_id, workspace_id)?;
    let page = workspace::create_page(&ws, &title, parent, ORIGIN_LOCAL)?;
    let doc = state.hub.get_or_load(&page, workspace_id)?;
    markdown::set_markdown(&doc, body, ORIGIN_LOCAL);
    state.sync.subscribe_all(workspace_id);
    Ok(page)
}

/// Imports one markdown file as a page, with the images beside it.
fn import_md_file(
    state: &AppState,
    workspace_id: &str,
    parent: &str,
    path: &std::path::Path,
) -> Result<String> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::msg(e.to_string()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Imported".into());
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let text = import_assets(state, workspace_id, dir, &text);
    import_markdown(state, workspace_id, parent, &name, &text)
}

/// How many opened files wait for a workspace to go into.
#[tauri::command]
pub fn pending_files(app: tauri::AppHandle) -> usize {
    crate::opened::count(&app)
}

/// Imports the markdown files opened with Kawruh (see opened.rs) as top-level
/// pages of `workspace_id`. Returns their ids.
#[tauri::command]
pub fn open_pending_files(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Vec<String>> {
    let mut pages = Vec::new();
    for path in crate::opened::take(&app) {
        match import_md_file(&state, &workspace_id, "", &path) {
            Ok(page) => pages.push(page),
            Err(e) => tracing::warn!("opening {}: {e}", path.display()),
        }
    }
    if !pages.is_empty() {
        tauri::async_runtime::spawn(assets::flush(app, workspace_id));
    }
    Ok(pages)
}

fn split_title<'a>(fallback: &str, md: &'a str) -> (String, &'a str) {
    let trimmed = md.trim_start();
    if let Some(rest) = trimmed.strip_prefix("# ") {
        let (line, body) = rest.split_once('\n').unwrap_or((rest, ""));
        return (line.trim().to_string(), body.trim_start_matches('\n'));
    }
    (fallback.to_string(), md)
}

/// Exports a page through a native save dialog run here, so the webview
/// never gets to name a path on disk. Returns the saved path, or None if cancelled.
#[tauri::command]
pub async fn page_export_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    page_id: String,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let markdown_text = page_export_markdown(state, workspace_id.clone(), page_id.clone())?;
    let title = markdown_text
        .strip_prefix("# ")
        .and_then(|rest| rest.lines().next())
        .unwrap_or("Untitled");
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(format!("{}.md", file_name(title)))
        .add_filter("Markdown", &["md"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| Error::msg(e.to_string()))?;
    let markdown_text = export_assets(&app, &path, &markdown_text).await?;
    std::fs::write(&path, markdown_text).map_err(|e| Error::msg(e.to_string()))?;
    Ok(Some(path.display().to_string()))
}

/// Copies the page's images into `<name>_assets/` beside the exported file and
/// points the markdown at them, so the export opens anywhere.
async fn export_assets(app: &tauri::AppHandle, path: &std::path::Path, md: &str) -> Result<String> {
    let mut refs = Vec::new();
    markdown::map_image_srcs(md, |src| {
        if let Some((ws, name)) = assets::parse_src(src) {
            refs.push((ws.to_string(), name.to_string()));
        }
        None
    });
    if refs.is_empty() {
        return Ok(md.to_string());
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "page".into());
    let folder = format!("{stem}_assets");
    let dir = path.with_file_name(&folder);
    let mut saved = std::collections::HashSet::new();
    for (ws, name) in refs {
        if !saved.insert(name.clone()) {
            continue;
        }
        if let Some((_, bytes)) = assets::load(app, &ws, &name).await? {
            std::fs::create_dir_all(&dir).map_err(|e| Error::msg(e.to_string()))?;
            std::fs::write(dir.join(&name), bytes).map_err(|e| Error::msg(e.to_string()))?;
        }
    }
    Ok(markdown::map_image_srcs(md, |src| {
        assets::parse_src(src)
            .filter(|(_, name)| saved.contains(*name))
            .map(|(_, name)| format!("{folder}/{name}"))
    }))
}

/// Stores images a markdown file refers to by relative path (an export's
/// `_assets/` folder, or any image next to it) and points the page at them.
fn import_assets(state: &AppState, workspace_id: &str, dir: &std::path::Path, md: &str) -> String {
    relink_local_images(md, dir, |bytes| {
        let name = assets::put(&state.db(), workspace_id, bytes, "").ok()?;
        Some(assets::src(workspace_id, &name))
    })
}

/// Rewrites relative image paths that point at a readable file under `dir`
/// with whatever `store` returns for the file's bytes.
pub(crate) fn relink_local_images(
    md: &str,
    dir: &std::path::Path,
    mut store: impl FnMut(&[u8]) -> Option<String>,
) -> String {
    markdown::map_image_srcs(md, |src| {
        if src.contains("://") || src.starts_with("data:") || src.starts_with('/') {
            return None;
        }
        let rel = percent_encoding::percent_decode_str(src).decode_utf8_lossy();
        let file = dir.join(rel.as_ref());
        let meta = std::fs::metadata(&file).ok()?;
        if !meta.is_file() || meta.len() as usize > assets::MAX_BYTES {
            return None;
        }
        store(&std::fs::read(&file).ok()?)
    })
}

/// Imports a markdown file picked in a native open dialog as a sub-page.
/// Returns the new page id, or None if cancelled.
#[tauri::command]
pub async fn page_import_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    parent: String,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app
        .dialog()
        .file()
        .add_filter("Markdown", &["md", "markdown", "txt"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| Error::msg(e.to_string()))?;
    let page = import_md_file(&state, &workspace_id, &parent, &path)?;
    tauri::async_runtime::spawn(assets::flush(app, workspace_id));
    Ok(Some(page))
}

/// A page title as a file name: path separators, reserved and control characters
/// become dashes, leading dots (hidden files) and trailing ones go, and long
/// titles are cut to fit file system limits.
pub(crate) fn file_name(title: &str) -> String {
    let mut name = String::new();
    for c in title.chars() {
        if name.len() + c.len_utf8() > 200 {
            break;
        }
        if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
            name.push('-');
        } else {
            name.push(c);
        }
    }
    let name = name.trim().trim_matches('.').trim();
    if name.is_empty() {
        "Untitled".into()
    } else {
        name.into()
    }
}

// ---------------------------------------------------- workspace import/export

const IMPORT_FOLDER: &str = "Folder…";
const IMPORT_ZIP: &str = "Zip file…";

/// Imports an Obsidian vault or a Notion export (a folder, or the .zip Notion
/// sends) as pages under `parent` ("" = top level), picked in native dialogs
/// run here. Returns None if cancelled.
#[tauri::command]
pub async fn workspace_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    parent: String,
) -> Result<Option<transfer::Imported>> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogResult};
    let choice = app
        .dialog()
        .message("Pick the vault or export folder, or the .zip file Notion sent you.")
        .title("Import Notion or Obsidian")
        .buttons(MessageDialogButtons::YesNoCancelCustom(
            IMPORT_FOLDER.into(),
            IMPORT_ZIP.into(),
            "Cancel".into(),
        ))
        .blocking_show_with_result();
    let picked = match choice {
        MessageDialogResult::Custom(c) if c == IMPORT_FOLDER => {
            app.dialog().file().blocking_pick_folder()
        }
        MessageDialogResult::Custom(c) if c == IMPORT_ZIP => app
            .dialog()
            .file()
            .add_filter("Zip", &["zip"])
            .blocking_pick_file(),
        _ => None,
    };
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| Error::msg(e.to_string()))?;
    let ws = state.hub.get_or_load(&workspace_id, &workspace_id)?;
    let imported = transfer::import(&ws, &parent, &path, |page, dir, md| {
        let md = import_assets(&state, &workspace_id, dir, md);
        let doc = state.hub.get_or_load(page, &workspace_id)?;
        markdown::set_markdown(&doc, &md, ORIGIN_LOCAL);
        Ok(())
    })?;
    state.sync.subscribe_all(&workspace_id);
    tauri::async_runtime::spawn(assets::flush(app, workspace_id));
    Ok(Some(imported))
}

/// Exports every live page into a new folder inside one picked in a native
/// dialog: `Title.md` per page, sub-pages in `Title/`, images in `_assets/`.
/// Returns the folder written, or None if cancelled.
#[tauri::command]
pub async fn workspace_export(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let Some(picked) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    let dest = picked.into_path().map_err(|e| Error::msg(e.to_string()))?;
    let ws = state.hub.get_or_load(&workspace_id, &workspace_id)?;
    let mut pages = Vec::new();
    for page in workspace::live_pages(&ws) {
        let doc = state.hub.get_or_load(&page.id, &workspace_id)?;
        pages.push((page, markdown::doc_to_markdown(&doc)));
    }
    let name = file_name(&workspace::workspace_name(&ws));
    let root = dest.join(transfer::dedupe(&name, |n| dest.join(n).exists()));
    let mut saved = std::collections::HashSet::new();
    for (ws_id, name) in pages.iter().flat_map(|(_, md)| transfer::asset_refs(md)) {
        if saved.contains(&name) {
            continue;
        }
        // An image that can't be fetched now keeps its kawruh-asset:// link.
        if let Ok(Some((_, bytes))) = assets::load(&app, &ws_id, &name).await {
            let dir = root.join(transfer::ASSETS_DIR);
            std::fs::create_dir_all(&dir).map_err(|e| Error::msg(e.to_string()))?;
            std::fs::write(dir.join(&name), bytes).map_err(|e| Error::msg(e.to_string()))?;
            saved.insert(name);
        }
    }
    transfer::export(&root, &pages, &saved)?;
    Ok(Some(root.display().to_string()))
}

// ------------------------------------------------------------------------ PDF

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrintPage {
    id: String,
    title: String,
    icon: String,
    /// 0 for the exported page, 1 for its sub-pages, and so on.
    depth: usize,
    markdown: String,
    /// Canvas id → SVG preview.
    canvases: std::collections::HashMap<String, String>,
    /// Link card href → metadata JSON.
    links: std::collections::HashMap<String, String>,
}

/// A page, and with `include_children` its whole sub-tree in outline order,
/// with what the webview needs to lay it out for printing.
#[tauri::command]
pub fn page_print_data(
    state: State<'_, AppState>,
    workspace_id: String,
    page_id: String,
    include_children: bool,
) -> Result<Vec<PrintPage>> {
    let ws = state.hub.get_or_load(&workspace_id, &workspace_id)?;
    let live = workspace::live_pages(&ws);
    if !live.iter().any(|p| p.id == page_id) {
        return Err(Error::msg("page not found"));
    }
    let mut order = Vec::new();
    outline(&live, &page_id, 0, include_children, &mut order);
    order
        .into_iter()
        .map(|(page, depth)| {
            let doc = state.hub.get_or_load(&page.id, &workspace_id)?;
            Ok(PrintPage {
                id: page.id.clone(),
                title: page.title.clone(),
                icon: page.icon.clone(),
                depth,
                markdown: markdown::doc_to_markdown(&doc),
                canvases: string_map(&doc, "canvas_svg"),
                links: string_map(&doc, "link_meta"),
            })
        })
        .collect()
}

/// Depth-first: a page, then each child's sub-tree in sibling order.
fn outline<'a>(
    live: &'a [workspace::PageMeta],
    id: &str,
    depth: usize,
    deep: bool,
    out: &mut Vec<(&'a workspace::PageMeta, usize)>,
) {
    let Some(page) = live.iter().find(|p| p.id == id) else {
        return;
    };
    out.push((page, depth));
    if deep {
        for child in live.iter().filter(|p| p.parent == id) {
            outline(live, &child.id, depth + 1, deep, out);
        }
    }
}

fn string_map(
    doc: &kawruh_core::yrs::Doc,
    name: &str,
) -> std::collections::HashMap<String, String> {
    use kawruh_core::yrs::{Any, Map, Out, Transact};
    let map = doc.get_or_insert_map(name);
    let txn = doc.transact();
    map.iter(&txn)
        .filter_map(|(k, v)| match v {
            Out::Any(Any::String(s)) => Some((k.to_string(), s.to_string())),
            _ => None,
        })
        .collect()
}

/// Saves the print layout the webview has just prepared: as a PDF file picked
/// in a save dialog on macOS (returns its path), or through the system print
/// dialog elsewhere (returns None).
#[tauri::command]
pub async fn page_export_pdf(app: tauri::AppHandle, title: String) -> Result<Option<String>> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_dialog::DialogExt;
        let Some(picked) = app
            .dialog()
            .file()
            .set_file_name(format!("{}.pdf", file_name(&title)))
            .add_filter("PDF", &["pdf"])
            .blocking_save_file()
        else {
            return Ok(None);
        };
        let path = picked.into_path().map_err(|e| Error::msg(e.to_string()))?;
        crate::pdf::print_to_pdf(&app, &path).await?;
        Ok(Some(path.display().to_string()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = title;
        crate::pdf::print_to_pdf(&app, std::path::Path::new("")).await?;
        Ok(None)
    }
}

// ----------------------------------------------------- search, links, history

#[tauri::command]
pub fn search_pages(
    state: State<'_, AppState>,
    workspace_id: String,
    query: String,
) -> Result<Vec<crate::search::SearchHit>> {
    state.search.search(&state.hub, &workspace_id, &query, 30)
}

/// Pages that link to `page_id` with `[…](kawruh://page/<id>)`.
#[tauri::command]
pub fn page_backlinks(
    state: State<'_, AppState>,
    workspace_id: String,
    page_id: String,
) -> Result<Vec<crate::search::PageRef>> {
    state.search.backlinks(&state.hub, &workspace_id, &page_id)
}

/// The workspace's live pages and the page links between them.
#[tauri::command]
pub fn workspace_graph(
    state: State<'_, AppState>,
    workspace_id: String,
) -> Result<crate::search::Graph> {
    state.search.graph(&state.hub, &workspace_id)
}

#[tauri::command]
pub fn page_history(
    state: State<'_, AppState>,
    page_id: String,
) -> Result<Vec<crate::history::Entry>> {
    crate::history::list(&state.db(), &page_id)
}

#[tauri::command]
pub fn page_history_get(state: State<'_, AppState>, page_id: String, id: i64) -> Result<String> {
    crate::history::get(&state.db(), &page_id, id)?
        .ok_or_else(|| Error::msg("that version is gone"))
}

/// Replaces the page with an earlier version. What's there now is saved first,
/// so a restore can itself be undone from the history.
#[tauri::command]
pub fn page_history_restore(
    state: State<'_, AppState>,
    workspace_id: String,
    page_id: String,
    id: i64,
) -> Result<()> {
    let markdown_text = crate::history::get(&state.db(), &page_id, id)?
        .ok_or_else(|| Error::msg("that version is gone"))?;
    let doc = state.hub.get_or_load(&page_id, &workspace_id)?;
    let current = markdown::doc_to_markdown(&doc);
    state
        .history
        .snapshot(&state.db(), &page_id, &current, workspace::now_ms())?;
    markdown::set_markdown(&doc, &markdown_text, ORIGIN_LOCAL);
    Ok(())
}

// -------------------------------------------------------------- images, links

/// Stores a pasted image. The webview sends the raw bytes as the request body
/// with `x-kawruh-workspace` and `x-kawruh-mime` headers; returns the `src`.
#[tauri::command]
pub fn asset_put(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: tauri::ipc::Request<'_>,
) -> Result<String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err(Error::msg("expected image bytes"));
    };
    let header = |k: &str| {
        request
            .headers()
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let workspace_id = header("x-kawruh-workspace");
    db::get_workspace(&state.db(), &workspace_id)?
        .ok_or_else(|| Error::msg("workspace not found"))?;
    let name = assets::put(&state.db(), &workspace_id, bytes, &header("x-kawruh-mime"))?;
    tauri::async_runtime::spawn(assets::flush(app, workspace_id.clone()));
    Ok(assets::src(&workspace_id, &name))
}

#[tauri::command]
pub async fn link_preview(url: String) -> Result<crate::linkmeta::LinkMeta> {
    crate::linkmeta::fetch(&url).await
}

#[tauri::command]
pub fn setting_get(state: State<'_, AppState>, key: String) -> Result<Option<String>> {
    db::get_setting(&state.db(), &key)
}

#[tauri::command]
pub fn setting_set(state: State<'_, AppState>, key: String, value: String) -> Result<()> {
    db::set_setting(&state.db(), &key, &value)
}

#[cfg(test)]
mod tests {
    use super::{file_name, outline, relink_local_images, split_title};
    use kawruh_core::workspace::PageMeta;

    fn page(id: &str, parent: &str, order: f64) -> PageMeta {
        PageMeta {
            id: id.into(),
            title: id.to_uppercase(),
            parent: parent.into(),
            order,
            icon: String::new(),
            created: 0,
            deleted: 0,
        }
    }

    #[test]
    fn print_outline_is_depth_first_in_sibling_order() {
        // live_pages already sorts by order.
        let live = vec![
            page("root", "", 1.0),
            page("a", "root", 1.0),
            page("other", "", 2.0),
            page("b", "root", 2.0),
            page("a1", "a", 1.0),
        ];
        let mut out = Vec::new();
        outline(&live, "root", 0, true, &mut out);
        let got: Vec<_> = out.iter().map(|(p, d)| (p.id.as_str(), *d)).collect();
        assert_eq!(got, vec![("root", 0), ("a", 1), ("a1", 2), ("b", 1)]);
        let mut shallow = Vec::new();
        outline(&live, "root", 0, false, &mut shallow);
        assert_eq!(shallow.len(), 1);
    }

    #[test]
    fn titles_become_safe_file_names() {
        assert_eq!(file_name("Q4: plan / v2?"), "Q4- plan - v2-");
        assert_eq!(file_name("   "), "Untitled");
        assert_eq!(file_name(".env notes."), "env notes");
        assert_eq!(file_name(&"é".repeat(300)).len(), 200);
    }

    #[test]
    fn imports_images_next_to_the_markdown_file() {
        let dir = std::env::temp_dir().join(format!("kawruh-import-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("My page_assets")).unwrap();
        std::fs::write(dir.join("My page_assets/a.png"), b"png bytes").unwrap();
        let md = "![a](<My page_assets/a.png>) ![b](missing.png) ![c](https://e.com/c.png)\n";
        let out = relink_local_images(md, &dir, |bytes| {
            assert_eq!(bytes, b"png bytes");
            Some("kawruh-asset://w/x.png".into())
        });
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(
            out,
            "![a](kawruh-asset://w/x.png) ![b](missing.png) ![c](https://e.com/c.png)\n"
        );
    }

    #[test]
    fn leading_heading_becomes_title() {
        assert_eq!(
            split_title("x", "# Hello\n\nbody"),
            ("Hello".into(), "body")
        );
        assert_eq!(split_title("x", "body only"), ("x".into(), "body only"));
    }
}
