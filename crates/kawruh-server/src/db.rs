//! Server-side relational data: users, tokens, invites, workspaces, members,
//! shares, and the derived `page_text` search index. Documents themselves live in the
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
        CREATE INDEX IF NOT EXISTS page_text_workspace ON page_text(workspace_id);
        CREATE TABLE IF NOT EXISTS invites (
            code_hash TEXT PRIMARY KEY,
            code TEXT NOT NULL,
            created_by TEXT REFERENCES users(id) ON DELETE CASCADE,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL
        );
        -- Stored assets no document refers to, and since when (see gc.rs).
        CREATE TABLE IF NOT EXISTS orphan_assets (
            workspace_id TEXT NOT NULL,
            name TEXT NOT NULL,
            since INTEGER NOT NULL,
            PRIMARY KEY (workspace_id, name)
        );
        -- One row per (comment, recipient): a mention or a reply in a thread
        -- someone else is part of.
        CREATE TABLE IF NOT EXISTS notifications (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            workspace_id TEXT NOT NULL,
            page_id TEXT NOT NULL,
            comment_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            body TEXT NOT NULL,
            author TEXT NOT NULL,
            author_name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            read_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS notifications_user ON notifications(user_id, created_at);",
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

pub fn user_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
}

/// How long an invite code stays valid.
pub const INVITE_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Serialize)]
pub struct Invite {
    pub code: String,
    pub created_at: i64,
    pub expires_at: i64,
}

/// Invite codes are looked up by their SHA-256, so checking one leaks nothing
/// about stored codes through timing. The code is kept too, for listing.
fn invite_hash(code: &str) -> String {
    crate::auth::hash_token(&crate::auth::normalize_invite(code))
}

/// A fresh single-use invite code valid for `INVITE_TTL_MS` from `now`.
/// `created_by` is `None` for a system invite (admin CLI). Prunes expired codes.
pub fn create_invite(
    conn: &Connection,
    created_by: Option<&str>,
    now: i64,
) -> rusqlite::Result<Invite> {
    conn.execute("DELETE FROM invites WHERE expires_at <= ?1", [now])?;
    let invite = Invite {
        code: crate::auth::new_invite_code(),
        created_at: now,
        expires_at: now + INVITE_TTL_MS,
    };
    conn.execute(
        "INSERT INTO invites (code_hash, code, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![invite_hash(&invite.code), invite.code, created_by, invite.created_at, invite.expires_at],
    )?;
    Ok(invite)
}

/// A user's unused, unexpired invite codes.
pub fn invites_of(conn: &Connection, user_id: &str, now: i64) -> rusqlite::Result<Vec<Invite>> {
    let mut stmt = conn.prepare(
        "SELECT code, created_at, expires_at FROM invites
         WHERE created_by = ?1 AND expires_at > ?2 ORDER BY created_at",
    )?;
    let rows = stmt.query_map(params![user_id, now], |r| {
        Ok(Invite {
            code: r.get(0)?,
            created_at: r.get(1)?,
            expires_at: r.get(2)?,
        })
    })?;
    rows.collect()
}

/// Revokes one of a user's invite codes; false when they have no such code.
pub fn delete_invite(conn: &Connection, user_id: &str, code: &str) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM invites WHERE code_hash = ?1 AND created_by = ?2",
        params![invite_hash(code), user_id],
    )?;
    Ok(n > 0)
}

/// Uses up an invite code; false when it is unknown, expired or already used.
pub fn consume_invite(conn: &Connection, code: &str, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "DELETE FROM invites WHERE code_hash = ?1 AND expires_at > ?2",
        params![invite_hash(code), now],
    )?;
    Ok(n > 0)
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

pub fn workspaces_owned_by(conn: &Connection, user_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM workspaces WHERE owner_id = ?1")?;
    let rows = stmt.query_map([user_id], |r| r.get(0))?;
    rows.collect()
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

// ---- notifications ----------------------------------------------------------

/// One thing a user should look at: a mention or a reply in a thread.
#[derive(Debug, Clone, Serialize)]
pub struct Notification {
    pub id: String,
    pub workspace_id: String,
    pub page_id: String,
    pub comment_id: String,
    /// "mention" or "reply".
    pub kind: String,
    pub body: String,
    pub author: String,
    pub author_name: String,
    pub created_at: i64,
    /// When the user read it; null while unread.
    pub read_at: Option<i64>,
}

/// A notification to record.
pub struct NewNotification<'a> {
    pub user_id: &'a str,
    pub workspace_id: &'a str,
    pub page_id: &'a str,
    pub comment_id: &'a str,
    /// "mention" or "reply".
    pub kind: &'a str,
    pub body: &'a str,
    pub author: &'a str,
    pub author_name: &'a str,
    pub created_at: i64,
}

/// Records a notification for `user_id`, unless it is already there.
/// The id is `<comment_id>:<user_id>`, so the same comment never notifies
/// the same person twice.
pub fn insert_notification(conn: &Connection, n: &NewNotification<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO notifications
           (id, user_id, workspace_id, page_id, comment_id, kind, body, author, author_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            format!("{}:{}", n.comment_id, n.user_id),
            n.user_id,
            n.workspace_id,
            n.page_id,
            n.comment_id,
            n.kind,
            n.body,
            n.author,
            n.author_name,
            n.created_at
        ],
    )?;
    Ok(())
}

/// The notifications of a user, newest first.
pub fn notifications_of(
    conn: &Connection,
    user_id: &str,
    limit: usize,
) -> rusqlite::Result<Vec<Notification>> {
    let mut stmt = conn.prepare(
        "SELECT id, workspace_id, page_id, comment_id, kind, body, author, author_name, created_at, read_at
         FROM notifications WHERE user_id = ?1 ORDER BY created_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![user_id, limit as i64], |r| {
        Ok(Notification {
            id: r.get(0)?,
            workspace_id: r.get(1)?,
            page_id: r.get(2)?,
            comment_id: r.get(3)?,
            kind: r.get(4)?,
            body: r.get(5)?,
            author: r.get(6)?,
            author_name: r.get(7)?,
            created_at: r.get(8)?,
            read_at: r.get(9)?,
        })
    })?;
    rows.collect()
}

/// How many notifications a user has not read.
pub fn unread_count(conn: &Connection, user_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM notifications WHERE user_id = ?1 AND read_at IS NULL",
        [user_id],
        |r| r.get(0),
    )
}

/// Marks a user's notifications as read; `ids` empty means all of them.
pub fn mark_read(
    conn: &Connection,
    user_id: &str,
    ids: &[String],
    now: i64,
) -> rusqlite::Result<usize> {
    if ids.is_empty() {
        return conn.execute(
            "UPDATE notifications SET read_at = ?2 WHERE user_id = ?1 AND read_at IS NULL",
            params![user_id, now],
        );
    }
    let mut n = 0;
    for id in ids {
        n += conn.execute(
            "UPDATE notifications SET read_at = ?3
             WHERE id = ?1 AND user_id = ?2 AND read_at IS NULL",
            params![id, user_id, now],
        )?;
    }
    Ok(n)
}

/// Removes the notifications of a comment (it was resolved or deleted).
pub fn drop_comment_notifications(conn: &Connection, comment_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM notifications WHERE comment_id = ?1",
        [comment_id],
    )?;
    Ok(())
}

/// Removes the notifications of a page (it was deleted).
pub fn drop_page_notifications(conn: &Connection, page_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM notifications WHERE page_id = ?1", [page_id])?;
    Ok(())
}

/// `(page_id, markdown)` of indexed, not deleted pages of a workspace whose
/// markdown contains `needle` (case-sensitive).
pub fn pages_containing(
    conn: &Connection,
    workspace_id: &str,
    needle: &str,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT page_id, markdown FROM page_text
         WHERE workspace_id = ?1 AND deleted = 0 AND instr(markdown, ?2) > 0",
    )?;
    let rows = stmt.query_map([workspace_id, needle], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("k.db")).unwrap();
        db.lock()
            .execute(
                "INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES ('u1','alice','Alice','x',1)",
                [],
            )
            .unwrap();
        (dir, db)
    }

    #[test]
    fn invites_are_single_use_and_expire() {
        let (_dir, db) = db();
        let conn = db.lock();
        let t0 = 1_000;
        let a = create_invite(&conn, Some("u1"), t0).unwrap();
        let b = create_invite(&conn, Some("u1"), t0).unwrap();
        assert_ne!(a.code, b.code);
        assert_eq!(a.expires_at, t0 + INVITE_TTL_MS);
        assert_eq!(invites_of(&conn, "u1", t0).unwrap().len(), 2);

        // Codes are case- and whitespace-insensitive, and work once.
        let typed = format!("  {}\n", a.code.to_lowercase());
        assert!(consume_invite(&conn, &typed, t0 + 1).unwrap());
        assert!(!consume_invite(&conn, &a.code, t0 + 1).unwrap());
        assert_eq!(invites_of(&conn, "u1", t0).unwrap().len(), 1);

        // Expired codes are neither listed nor accepted.
        let late = t0 + INVITE_TTL_MS;
        assert!(invites_of(&conn, "u1", late).unwrap().is_empty());
        assert!(!consume_invite(&conn, &b.code, late).unwrap());
        assert!(consume_invite(&conn, &b.code, late - 1).unwrap());
        assert!(!consume_invite(&conn, "KWR-NOPE-NOPE", t0).unwrap());

        // System invites have no owner; only the owner can revoke their codes.
        let sys = create_invite(&conn, None, t0).unwrap();
        assert!(!delete_invite(&conn, "u1", &sys.code).unwrap());
        let c = create_invite(&conn, Some("u1"), t0).unwrap();
        assert!(!delete_invite(&conn, "u2", &c.code).unwrap());
        assert!(delete_invite(&conn, "u1", &c.code).unwrap());
        assert!(!consume_invite(&conn, &c.code, t0).unwrap());
        assert!(consume_invite(&conn, &sys.code, t0).unwrap());
    }
}
