//! App-local tables (accounts, workspaces) next to the core document tables.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::Result;

pub fn migrate(conn: &Connection) -> Result<()> {
    kawruh_core::store::migrate(conn)?;
    crate::assets::migrate(conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS accounts (
            id TEXT PRIMARY KEY,
            server_url TEXT NOT NULL,
            user_id TEXT NOT NULL,
            username TEXT NOT NULL,
            display_name TEXT NOT NULL,
            token TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspaces (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            account_id TEXT,
            role TEXT NOT NULL DEFAULT 'owner',
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS settings (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub server_url: String,
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    #[serde(skip)]
    pub token: String,
}

fn account_row(r: &rusqlite::Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        server_url: r.get(1)?,
        user_id: r.get(2)?,
        username: r.get(3)?,
        display_name: r.get(4)?,
        token: r.get(5)?,
    })
}

const ACCOUNT_COLS: &str = "id, server_url, user_id, username, display_name, token";

pub fn list_accounts(conn: &Connection) -> Result<Vec<Account>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ACCOUNT_COLS} FROM accounts ORDER BY created_at"
    ))?;
    let rows = stmt.query_map([], account_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get_account(conn: &Connection, id: &str) -> Result<Option<Account>> {
    Ok(conn
        .query_row(
            &format!("SELECT {ACCOUNT_COLS} FROM accounts WHERE id = ?1"),
            [id],
            account_row,
        )
        .optional()?)
}

/// Inserts or refreshes the account for (server_url, user_id).
pub fn upsert_account(conn: &Connection, a: &Account) -> Result<Account> {
    let existing: Option<String> = conn
        .query_row(
            "SELECT id FROM accounts WHERE server_url = ?1 AND user_id = ?2",
            params![a.server_url, a.user_id],
            |r| r.get(0),
        )
        .optional()?;
    let id = existing.unwrap_or_else(|| a.id.clone());
    conn.execute(
        "INSERT INTO accounts (id, server_url, user_id, username, display_name, token, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET username = ?4, display_name = ?5, token = ?6",
        params![
            id,
            a.server_url,
            a.user_id,
            a.username,
            a.display_name,
            a.token,
            kawruh_core::workspace::now_ms()
        ],
    )?;
    Ok(Account { id, ..a.clone() })
}

/// Logging out keeps the row (workspaces still point at it) but drops the
/// token; logging in again to the same server/user reuses the row.
pub fn clear_token(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE accounts SET token = '' WHERE id = ?1", [id])?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub account_id: Option<String>,
    pub role: String,
    pub server_url: Option<String>,
    pub username: Option<String>,
}

fn workspace_row(r: &rusqlite::Row) -> rusqlite::Result<WorkspaceRow> {
    Ok(WorkspaceRow {
        id: r.get(0)?,
        name: r.get(1)?,
        account_id: r.get(2)?,
        role: r.get(3)?,
        server_url: r.get(4)?,
        username: r.get(5)?,
    })
}

const WORKSPACE_SELECT: &str = "SELECT w.id, w.name, w.account_id, w.role, a.server_url, a.username
     FROM workspaces w LEFT JOIN accounts a ON a.id = w.account_id";

pub fn list_workspaces(conn: &Connection) -> Result<Vec<WorkspaceRow>> {
    let mut stmt = conn.prepare(&format!("{WORKSPACE_SELECT} ORDER BY w.created_at"))?;
    let rows = stmt.query_map([], workspace_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get_workspace(conn: &Connection, id: &str) -> Result<Option<WorkspaceRow>> {
    Ok(conn
        .query_row(
            &format!("{WORKSPACE_SELECT} WHERE w.id = ?1"),
            [id],
            workspace_row,
        )
        .optional()?)
}

pub fn insert_workspace(
    conn: &Connection,
    id: &str,
    name: &str,
    account_id: Option<&str>,
    role: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO workspaces (id, name, account_id, role, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET account_id = ?3, role = ?4",
        params![id, name, account_id, role, kawruh_core::workspace::now_ms()],
    )?;
    Ok(())
}

pub fn set_workspace_name(conn: &Connection, id: &str, name: &str) -> Result<()> {
    conn.execute(
        "UPDATE workspaces SET name = ?2 WHERE id = ?1 AND name != ?2",
        params![id, name],
    )?;
    Ok(())
}

pub fn set_workspace_role(conn: &Connection, id: &str, role: &str) -> Result<()> {
    conn.execute(
        "UPDATE workspaces SET role = ?2 WHERE id = ?1",
        params![id, role],
    )?;
    Ok(())
}

pub fn delete_workspace(conn: &mut Connection, id: &str) -> Result<()> {
    kawruh_core::store::delete_workspace_docs(conn, id)?;
    conn.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![key, value],
    )?;
    Ok(())
}
