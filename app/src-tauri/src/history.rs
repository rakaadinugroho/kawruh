//! Page history: markdown snapshots kept on this device. A page is
//! snapshotted when it's opened and while it changes, at most every ten
//! minutes, keeping the newest fifty. Restoring one is an ordinary edit, so it
//! syncs like any other.

use std::collections::HashMap;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::error::Result;

pub const EVERY_MS: i64 = 10 * 60 * 1000;
pub const KEEP: i64 = 50;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS page_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            page_id TEXT NOT NULL,
            ts INTEGER NOT NULL,
            markdown TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS page_history_page ON page_history (page_id, ts);",
    )?;
    Ok(())
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: i64,
    pub ts: i64,
    /// The start of the page's text, to tell versions apart at a glance.
    pub preview: String,
}

/// When each page was last snapshotted, so most edits skip the database.
#[derive(Default)]
pub struct History {
    last: Mutex<HashMap<String, i64>>,
}

impl History {
    /// True when `page_id` is due for a snapshot at `now`.
    pub fn due(&self, conn: &Connection, page_id: &str, now: i64) -> Result<bool> {
        if let Some(ts) = self.last.lock().unwrap().get(page_id) {
            return Ok(now - ts >= EVERY_MS);
        }
        let ts = latest(conn, page_id)?.map(|(ts, _)| ts);
        if let Some(ts) = ts {
            self.last.lock().unwrap().insert(page_id.to_string(), ts);
        }
        Ok(ts.is_none_or(|ts| now - ts >= EVERY_MS))
    }

    /// Stores `markdown` unless it equals the newest snapshot, then trims old ones.
    pub fn snapshot(
        &self,
        conn: &Connection,
        page_id: &str,
        markdown: &str,
        now: i64,
    ) -> Result<()> {
        self.last.lock().unwrap().insert(page_id.to_string(), now);
        if latest(conn, page_id)?.is_some_and(|(_, md)| md == markdown)
            || markdown.trim().is_empty()
        {
            return Ok(());
        }
        conn.execute(
            "INSERT INTO page_history (page_id, ts, markdown) VALUES (?1, ?2, ?3)",
            params![page_id, now, markdown],
        )?;
        conn.execute(
            "DELETE FROM page_history WHERE page_id = ?1 AND id NOT IN
               (SELECT id FROM page_history WHERE page_id = ?1 ORDER BY ts DESC, id DESC LIMIT ?2)",
            params![page_id, KEEP],
        )?;
        Ok(())
    }
}

fn latest(conn: &Connection, page_id: &str) -> Result<Option<(i64, String)>> {
    Ok(conn
        .query_row(
            "SELECT ts, markdown FROM page_history WHERE page_id = ?1 ORDER BY ts DESC, id DESC LIMIT 1",
            [page_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Newest first.
pub fn list(conn: &Connection, page_id: &str) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, markdown FROM page_history WHERE page_id = ?1 ORDER BY ts DESC, id DESC",
    )?;
    let rows = stmt.query_map([page_id], |r| {
        let markdown: String = r.get(2)?;
        Ok(Entry {
            id: r.get(0)?,
            ts: r.get(1)?,
            preview: crate::search::plain_text(&markdown)
                .chars()
                .take(120)
                .collect::<String>()
                .replace('\n', " · "),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// A snapshot's markdown, if it belongs to `page_id`.
pub fn get(conn: &Connection, page_id: &str, id: i64) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT markdown FROM page_history WHERE id = ?1 AND page_id = ?2",
            params![id, page_id],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn snapshots_at_most_every_ten_minutes_and_skip_repeats() {
        let conn = db();
        let h = History::default();
        assert!(h.due(&conn, "p", 0).unwrap());
        h.snapshot(&conn, "p", "one", 0).unwrap();
        assert!(!h.due(&conn, "p", EVERY_MS - 1).unwrap());
        assert!(h.due(&conn, "p", EVERY_MS).unwrap());
        h.snapshot(&conn, "p", "one", EVERY_MS).unwrap(); // unchanged: not stored
        h.snapshot(&conn, "p", "two", 2 * EVERY_MS).unwrap();
        let entries = list(&conn, "p").unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].preview, "two");
        assert_eq!(
            get(&conn, "p", entries[1].id).unwrap().as_deref(),
            Some("one")
        );
        assert_eq!(get(&conn, "other", entries[1].id).unwrap(), None);
    }

    #[test]
    fn keeps_only_the_newest_fifty() {
        let conn = db();
        let h = History::default();
        for i in 0..(KEEP + 5) {
            h.snapshot(&conn, "p", &format!("v{i}"), i * EVERY_MS)
                .unwrap();
        }
        let entries = list(&conn, "p").unwrap();
        assert_eq!(entries.len() as i64, KEEP);
        assert_eq!(entries[0].preview, format!("v{}", KEEP + 4));
    }

    #[test]
    fn a_fresh_history_reads_the_last_snapshot_time_from_the_database() {
        let conn = db();
        History::default()
            .snapshot(&conn, "p", "one", 1000)
            .unwrap();
        let reopened = History::default();
        assert!(!reopened.due(&conn, "p", 2000).unwrap());
        assert!(reopened.due(&conn, "p", 1000 + EVERY_MS).unwrap());
    }
}
