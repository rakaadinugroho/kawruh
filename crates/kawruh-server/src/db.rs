//! Server-side relational data: users, tokens, workspaces, members, shares,
//! and the derived `page_text` search index. Documents themselves live in the
//! `kawruh_core::store` tables of the same database.

use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use kawruh_core::workspace::now_ms;

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &std::path::Path) -> anyhow::Result<Db> {
        let conn = kawruh_core::store::open(path)?;
        migrate(&conn)?;
        Ok(Db(Arc::new(Mutex::new(conn))))
    }

    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            username TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tokens (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            kind TEXT NOT NULL,
            name TEXT NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            created_at INTEGER NOT NULL,
            last_used_at INTEGER
        );
        CREATE TABLE IF NOT EXISTS workspaces (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            owner_id TEXT NOT NULL REFERENCES users(id),
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS members (
            workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            role TEXT NOT NULL,
            PRIMARY KEY (workspace_id, user_id)
        );
        CREATE TABLE IF NOT EXISTS shares (
            token TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
            page_id TEXT NOT NULL,
            created_by TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS page_text (
            page_id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            markdown TEXT NOT NULL DEFAULT '',
            deleted INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS page_text_workspace ON page_text(workspace_id);",
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Editor,
    Owner,
}

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        match s {
            "viewer" => Some(Role::Viewer),
            "editor" => Some(Role::Editor),
            "owner" => Some(Role::Owner),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Owner => "owner",
        }
    }
    pub fn can_edit(self) -> bool {
        self >= Role::Editor
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRow {
    pub id: String,
    pub name: String,
    pub role: Role,
}

#[derive(Debug, Clone, Serialize)]
pub struct Member {
    pub user_id: String,
    pub username: String,
    pub display_name: String,
    pub role: Role,
}

#[derive(Debug, Clone)]
pub struct Share {
    pub token: String,
    pub workspace_id: String,
    pub page_id: String,
    pub created_at: i64,
}

pub fn user_by_username(
    conn: &Connection,
    username: &str,
) -> rusqlite::Result<Option<(User, String)>> {
    conn.query_row(
        "SELECT id, username, display_name, password_hash FROM users WHERE username = ?1",
        [username],
        |r| {
            Ok((
                User {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    display_name: r.get(2)?,
                },
                r.get(3)?,
            ))
        },
    )
    .optional()
}

pub fn insert_user(conn: &Connection, user: &User, password_hash: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![user.id, user.username, user.display_name, password_hash, now_ms()],
    )?;
    Ok(())
}

pub struct TokenRow {
    pub id: String,
    pub kind: String,
    pub user: User,
}

pub fn insert_token(
    conn: &Connection,
    id: &str,
    user_id: &str,
    kind: &str,
    name: &str,
    token_hash: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO tokens (id, user_id, kind, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, user_id, kind, name, token_hash, now_ms()],
    )?;
    Ok(())
}

pub fn token_by_hash(conn: &Connection, token_hash: &str) -> rusqlite::Result<Option<TokenRow>> {
    let row = conn
        .query_row(
            "SELECT t.id, t.kind, u.id, u.username, u.display_name
             FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = ?1",
            [token_hash],
            |r| {
                Ok(TokenRow {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    user: User {
                        id: r.get(2)?,
                        username: r.get(3)?,
                        display_name: r.get(4)?,
                    },
                })
            },
        )
        .optional()?;
    if let Some(t) = &row {
        conn.execute(
            "UPDATE tokens SET last_used_at = ?2 WHERE id = ?1",
            params![t.id, now_ms()],
        )?;
    }
    Ok(row)
}

pub fn role_of(
    conn: &Connection,
    workspace_id: &str,
    user_id: &str,
) -> rusqlite::Result<Option<Role>> {
    let role: Option<String> = conn
        .query_row(
            "SELECT role FROM members WHERE workspace_id = ?1 AND user_id = ?2",
            [workspace_id, user_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(role.as_deref().and_then(Role::parse))
}

pub fn workspace_owner(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT owner_id FROM workspaces WHERE id = ?1",
        [workspace_id],
        |r| r.get(0),
    )
    .optional()
}

pub fn workspaces_of(conn: &Connection, user_id: &str) -> rusqlite::Result<Vec<WorkspaceRow>> {
    let mut stmt = conn.prepare(
        "SELECT w.id, w.name, m.role FROM workspaces w
         JOIN members m ON m.workspace_id = w.id
         WHERE m.user_id = ?1 ORDER BY w.created_at",
    )?;
    let rows = stmt.query_map([user_id], |r| {
        let role: String = r.get(2)?;
        Ok(WorkspaceRow {
            id: r.get(0)?,
            name: r.get(1)?,
            role: Role::parse(&role).unwrap_or(Role::Viewer),
        })
    })?;
    rows.collect()
}

pub fn members_of(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<Member>> {
    let mut stmt = conn.prepare(
        "SELECT u.id, u.username, u.display_name, m.role FROM members m
         JOIN users u ON u.id = m.user_id WHERE m.workspace_id = ?1 ORDER BY u.username",
    )?;
    let rows = stmt.query_map([workspace_id], |r| {
        let role: String = r.get(3)?;
        Ok(Member {
            user_id: r.get(0)?,
            username: r.get(1)?,
            display_name: r.get(2)?,
            role: Role::parse(&role).unwrap_or(Role::Viewer),
        })
    })?;
    rows.collect()
}

pub fn set_workspace_name(
    conn: &Connection,
    workspace_id: &str,
    name: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE workspaces SET name = ?2 WHERE id = ?1 AND name != ?2",
        params![workspace_id, name],
    )?;
    Ok(())
}

pub fn share_by_token(conn: &Connection, token: &str) -> rusqlite::Result<Option<Share>> {
    conn.query_row(
        "SELECT token, workspace_id, page_id, created_at FROM shares WHERE token = ?1",
        [token],
        |r| {
            Ok(Share {
                token: r.get(0)?,
                workspace_id: r.get(1)?,
                page_id: r.get(2)?,
                created_at: r.get(3)?,
            })
        },
    )
    .optional()
}

pub fn shares_of(conn: &Connection, workspace_id: &str) -> rusqlite::Result<Vec<Share>> {
    let mut stmt = conn.prepare(
        "SELECT token, workspace_id, page_id, created_at FROM shares WHERE workspace_id = ?1 ORDER BY created_at",
    )?;
    let rows = stmt.query_map([workspace_id], |r| {
        Ok(Share {
            token: r.get(0)?,
            workspace_id: r.get(1)?,
            page_id: r.get(2)?,
            created_at: r.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn upsert_page_text(
    conn: &Connection,
    page_id: &str,
    workspace_id: &str,
    title: Option<&str>,
    markdown: Option<&str>,
    deleted: Option<bool>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO page_text (page_id, workspace_id, title, markdown, deleted, updated_at)
         VALUES (?1, ?2, COALESCE(?3, ''), COALESCE(?4, ''), COALESCE(?5, 0), ?6)
         ON CONFLICT(page_id) DO UPDATE SET
            title = COALESCE(?3, title),
            markdown = COALESCE(?4, markdown),
            deleted = COALESCE(?5, deleted),
            updated_at = ?6",
        params![page_id, workspace_id, title, markdown, deleted, now_ms()],
    )?;
    Ok(())
}

pub fn page_workspace(conn: &Connection, page_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT workspace_id FROM page_text WHERE page_id = ?1",
        [page_id],
        |r| r.get(0),
    )
    .optional()
}

#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub page_id: String,
    pub workspace_id: String,
    pub title: String,
    pub snippet: String,
}

/// Case-insensitive substring search over titles and markdown of live pages
/// in the given workspaces.
pub fn search(
    conn: &Connection,
    workspace_ids: &[String],
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<SearchHit>> {
    if workspace_ids.is_empty() || query.trim().is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = (0..workspace_ids.len())
        .map(|i| format!("?{}", i + 2))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT page_id, workspace_id, title, markdown FROM page_text
         WHERE deleted = 0 AND workspace_id IN ({placeholders})
           AND (instr(lower(title), lower(?1)) > 0 OR instr(lower(markdown), lower(?1)) > 0)
         ORDER BY instr(lower(title), lower(?1)) = 0, updated_at DESC
         LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut values: Vec<&dyn rusqlite::ToSql> = vec![&query];
    for id in workspace_ids {
        values.push(id);
    }
    let rows = stmt.query_map(values.as_slice(), |r| {
        let markdown: String = r.get(3)?;
        Ok(SearchHit {
            page_id: r.get(0)?,
            workspace_id: r.get(1)?,
            title: r.get(2)?,
            snippet: snippet(&markdown, query),
        })
    })?;
    rows.collect()
}

fn snippet(text: &str, query: &str) -> String {
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    let Some(pos) = lower.find(&q) else {
        return text
            .chars()
            .take(160)
            .collect::<String>()
            .replace('\n', " ");
    };
    // `lower` may differ in byte length from `text` for some scripts; map via chars.
    let char_pos = lower[..pos].chars().count();
    let start = char_pos.saturating_sub(60);
    text.chars()
        .skip(start)
        .take(160)
        .collect::<String>()
        .replace('\n', " ")
}
