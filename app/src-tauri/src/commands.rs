//! Tauri commands: the whole webview ↔ Rust surface (see app/src/lib/ipc.ts).

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use kawruh_core::{markdown, workspace};
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use crate::db::{self, Account, WorkspaceRow};
use crate::error::{Error, Result};
use crate::hub::{DocEvent, DocHub, ORIGIN_LOCAL};
use crate::remote::{ApiToken, Member, Remote, RemoteWorkspace, Share, normalize_server};
use crate::sync::SyncManager;

pub struct AppState {
    pub hub: Arc<DocHub>,
    pub sync: Arc<SyncManager>,
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
    fn workspace_account(&self, workspace_id: &str) -> Result<Account> {
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
    state
        .remote
        .create_share(&a.server_url, &a.token, &workspace_id, &page_id)
        .await
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
    let (title, body) = split_title(&title, &markdown_text);
    let ws = state.hub.get_or_load(&workspace_id, &workspace_id)?;
    let page = workspace::create_page(&ws, &title, &parent, ORIGIN_LOCAL)?;
    let doc = state.hub.get_or_load(&page, &workspace_id)?;
    markdown::set_markdown(&doc, body, ORIGIN_LOCAL);
    state.sync.subscribe_all(&workspace_id);
    Ok(page)
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
    let file_name: String = title
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(format!("{}.md", file_name.trim()))
        .add_filter("Markdown", &["md"])
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| Error::msg(e.to_string()))?;
    std::fs::write(&path, markdown_text).map_err(|e| Error::msg(e.to_string()))?;
    Ok(Some(path.display().to_string()))
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
    let text = std::fs::read_to_string(&path).map_err(|e| Error::msg(e.to_string()))?;
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Imported".into());
    page_import_markdown(state, workspace_id, parent, name, text).map(Some)
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
    use super::split_title;

    #[test]
    fn leading_heading_becomes_title() {
        assert_eq!(
            split_title("x", "# Hello\n\nbody"),
            ("Hello".into(), "body")
        );
        assert_eq!(split_title("x", "body only"), ("x".into(), "body only"));
    }
}
