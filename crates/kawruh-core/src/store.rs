//! SQLite persistence for Yjs documents, shared by the desktop app and server.
//!
//! Every update is appended durably (`doc_updates`); when a document has
//! accumulated enough of them it is compacted into a single state snapshot
//! (`docs.state`).

use rusqlite::{Connection, OptionalExtension, params};
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

/// Updates kept before a document is folded into its snapshot.
pub const COMPACT_AFTER: i64 = 200;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error("corrupt update for {0}: {1}")]
    Corrupt(String, String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Workspace,
    Page,
}

impl DocKind {
    fn as_str(self) -> &'static str {
        match self {
            DocKind::Workspace => "workspace",
            DocKind::Page => "page",
        }
    }
}

pub fn open(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS docs (
            id TEXT PRIMARY KEY,
            workspace_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            state BLOB,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS docs_workspace ON docs(workspace_id);
        CREATE TABLE IF NOT EXISTS doc_updates (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            doc_id TEXT NOT NULL,
            data BLOB NOT NULL
        );
        CREATE INDEX IF NOT EXISTS doc_updates_doc ON doc_updates(doc_id);",
    )?;
    Ok(())
}

/// Registers a document. No-op when it already exists.
pub fn ensure_doc(
    conn: &Connection,
    doc_id: &str,
    workspace_id: &str,
    kind: DocKind,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO docs (id, workspace_id, kind, state, updated_at) VALUES (?1, ?2, ?3, NULL, ?4)",
        params![doc_id, workspace_id, kind.as_str(), crate::workspace::now_ms()],
    )?;
    Ok(())
}

/// Workspace a document belongs to, if it is registered.
pub fn doc_workspace(conn: &Connection, doc_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT workspace_id FROM docs WHERE id = ?1",
            [doc_id],
            |r| r.get(0),
        )
        .optional()?)
}

fn apply(doc: &Doc, doc_id: &str, bytes: &[u8]) -> Result<()> {
    let update =
        Update::decode_v1(bytes).map_err(|e| StoreError::Corrupt(doc_id.into(), e.to_string()))?;
    doc.transact_mut()
        .apply_update(update)
        .map_err(|e| StoreError::Corrupt(doc_id.into(), e.to_string()))
}

/// Loads a document (snapshot + pending updates). `None` if never stored.
pub fn load_doc(conn: &Connection, doc_id: &str) -> Result<Option<Doc>> {
    let state: Option<Option<Vec<u8>>> = conn
        .query_row("SELECT state FROM docs WHERE id = ?1", [doc_id], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(state) = state else {
        return Ok(None);
    };
    let doc = Doc::new();
    if let Some(state) = state {
        apply(&doc, doc_id, &state)?;
    }
    let mut stmt =
        conn.prepare_cached("SELECT data FROM doc_updates WHERE doc_id = ?1 ORDER BY seq")?;
    let rows = stmt.query_map([doc_id], |r| r.get::<_, Vec<u8>>(0))?;
    for row in rows {
        apply(&doc, doc_id, &row?)?;
    }
    Ok(Some(doc))
}

/// Appends an update. Returns how many updates are pending compaction.
pub fn append_update(conn: &Connection, doc_id: &str, data: &[u8]) -> Result<i64> {
    conn.execute(
        "INSERT INTO doc_updates (doc_id, data) VALUES (?1, ?2)",
        params![doc_id, data],
    )?;
    conn.execute(
        "UPDATE docs SET updated_at = ?2 WHERE id = ?1",
        params![doc_id, crate::workspace::now_ms()],
    )?;
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM doc_updates WHERE doc_id = ?1",
        [doc_id],
        |r| r.get(0),
    )?)
}

/// Replaces snapshot + updates with the document's full current state.
pub fn compact(conn: &mut Connection, doc_id: &str, doc: &Doc) -> Result<()> {
    let state = doc
        .transact()
        .encode_state_as_update_v1(&StateVector::default());
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE docs SET state = ?2, updated_at = ?3 WHERE id = ?1",
        params![doc_id, state, crate::workspace::now_ms()],
    )?;
    tx.execute("DELETE FROM doc_updates WHERE doc_id = ?1", [doc_id])?;
    tx.commit()?;
    Ok(())
}

/// Document ids of a workspace, with their kind.
pub fn list_docs(conn: &Connection, workspace_id: &str) -> Result<Vec<(String, DocKind)>> {
    let mut stmt = conn.prepare("SELECT id, kind FROM docs WHERE workspace_id = ?1")?;
    let rows = stmt.query_map([workspace_id], |r| {
        let kind: String = r.get(1)?;
        Ok((
            r.get::<_, String>(0)?,
            if kind == "workspace" {
                DocKind::Workspace
            } else {
                DocKind::Page
            },
        ))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn delete_workspace_docs(conn: &mut Connection, workspace_id: &str) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM doc_updates WHERE doc_id IN (SELECT id FROM docs WHERE workspace_id = ?1)",
        [workspace_id],
    )?;
    tx.execute("DELETE FROM docs WHERE workspace_id = ?1", [workspace_id])?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{GetString, Text};

    #[test]
    fn append_load_compact() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        ensure_doc(&conn, "d", "w", DocKind::Page).unwrap();
        assert!(load_doc(&conn, "missing").unwrap().is_none());

        let doc = Doc::new();
        let text = doc.get_or_insert_text("t");
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let tx = std::sync::Mutex::new(tx);
        doc.observe_update_v1("test", move |_, e| {
            tx.lock().unwrap().send(e.update.clone()).unwrap()
        })
        .unwrap();
        text.insert(&mut doc.transact_mut(), 0, "hello");
        text.insert(&mut doc.transact_mut(), 5, " world");
        for u in rx.try_iter() {
            append_update(&conn, "d", &u).unwrap();
        }

        let loaded = load_doc(&conn, "d").unwrap().unwrap();
        let t = loaded.get_or_insert_text("t");
        assert_eq!(t.get_string(&loaded.transact()), "hello world");

        compact(&mut conn, "d", &loaded).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM doc_updates", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        let again = load_doc(&conn, "d").unwrap().unwrap();
        let t = again.get_or_insert_text("t");
        assert_eq!(t.get_string(&again.transact()), "hello world");
        assert_eq!(doc_workspace(&conn, "d").unwrap().as_deref(), Some("w"));
        assert_eq!(list_docs(&conn, "w").unwrap().len(), 1);
    }
}
