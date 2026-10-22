//! Notifications from page comments.
//!
//! A comment notifies the people it mentions and the people in its thread.
//! The rows are keyed by `<comment id>:<user id>`, so re-running this never
//! notifies the same person about the same comment twice, and a comment that
//! is edited to mention someone new does notify that person.

use std::collections::HashMap;

use rusqlite::Connection;
use yrs::Doc;

use kawruh_core::comments::{self, Comment};

use crate::db;

/// The users a comment notifies: the ones it mentions and, for a reply, the
/// others in its thread. The author is never notified about their own
/// comment.
fn recipients(all: &[Comment], comment: &Comment) -> Vec<(String, &'static str)> {
    let mut out: Vec<(String, &'static str)> = Vec::new();
    for user in comment.mentions() {
        if user != comment.author && !out.iter().any(|(u, _)| u == &user) {
            out.push((user, "mention"));
        }
    }
    // Only a reply reaches the people already in the thread; the comment that
    // started it does not announce itself to whoever answers later.
    if comment.id == comment.thread {
        return out;
    }
    for other in all {
        if other.thread != comment.thread || other.author.is_empty() {
            continue;
        }
        if other.author == comment.author {
            continue;
        }
        if !out.iter().any(|(u, _)| u == &other.author) {
            out.push((other.author.clone(), "reply"));
        }
    }
    out
}

/// Brings the notifications of a page in line with its comments: adds the ones
/// that are missing and removes the ones whose comment is gone or resolved.
pub fn sync_page_notifications(
    conn: &Connection,
    workspace_id: &str,
    page_id: &str,
    doc: &Doc,
) -> rusqlite::Result<()> {
    let all = comments::read(doc);
    let by_id: HashMap<&str, &Comment> = all.iter().map(|c| (c.id.as_str(), c)).collect();
    let resolved = |c: &Comment| {
        c.resolved
            || by_id
                .get(c.thread.as_str())
                .is_some_and(|root| root.resolved)
    };

    // Notifications of comments that are gone or done with.
    let known: Vec<String> = {
        let mut stmt =
            conn.prepare("SELECT DISTINCT comment_id FROM notifications WHERE page_id = ?1")?;
        let rows = stmt.query_map([page_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for comment_id in known {
        let gone = match by_id.get(comment_id.as_str()) {
            None => true,
            Some(c) => resolved(c),
        };
        if gone {
            db::drop_comment_notifications(conn, &comment_id)?;
        }
    }

    for comment in &all {
        if resolved(comment) {
            continue;
        }
        for (user_id, kind) in recipients(&all, comment) {
            // Only people in the workspace can be notified about it.
            if db::role_of(conn, workspace_id, &user_id)?.is_none() {
                continue;
            }
            db::insert_notification(
                conn,
                &db::NewNotification {
                    user_id: &user_id,
                    workspace_id,
                    page_id,
                    comment_id: &comment.id,
                    kind,
                    body: &comment.body,
                    author: &comment.author,
                    author_name: &comment.author_name,
                    created_at: comment.created,
                },
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kawruh_core::comments::Comment;

    struct Env {
        _dir: tempfile::TempDir,
        db: crate::db::Db,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(&dir.path().join("k.db")).unwrap();
        {
            let conn = db.lock();
            conn.execute(
                "INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES ('u1','alice','Alice','x',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES ('u2','bob','Bob','x',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO users (id, username, display_name, password_hash, created_at) VALUES ('u3','carol','Carol','x',1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO workspaces (id, name, owner_id, created_at) VALUES ('w1','W','u1',1)",
                [],
            )
            .unwrap();
            for (user, role) in [("u1", "owner"), ("u2", "editor")] {
                conn.execute(
                    "INSERT INTO members (workspace_id, user_id, role) VALUES ('w1', ?1, ?2)",
                    rusqlite::params![user, role],
                )
                .unwrap();
            }
        }
        Env { _dir: dir, db }
    }

    fn comment(id: &str, thread: &str, body: &str, author: &str, created: i64) -> Comment {
        let mut c = Comment::new(id, body, author, author, created);
        c.thread = thread.to_string();
        c
    }

    fn notifications(conn: &Connection, user: &str) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(
                "SELECT comment_id, kind FROM notifications WHERE user_id = ?1 ORDER BY comment_id",
            )
            .unwrap();
        let rows = stmt
            .query_map([user], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    #[test]
    fn a_mention_notifies_the_mentioned_member_only() {
        let e = env();
        let conn = e.db.lock();
        let doc = Doc::new();
        comments::write(
            &doc,
            &comment(
                "c1",
                "c1",
                "hi @[Bob](kawruh://user/u2) and @[Carol](kawruh://user/u3)",
                "u1",
                10,
            ),
        );
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        // Bob is a member, Carol is not.
        assert_eq!(
            notifications(&conn, "u2"),
            vec![("c1".into(), "mention".into())]
        );
        assert!(notifications(&conn, "u3").is_empty());
        // The author is not notified about their own comment.
        assert!(notifications(&conn, "u1").is_empty());
    }

    #[test]
    fn a_reply_notifies_the_thread_and_nobody_twice() {
        let e = env();
        let conn = e.db.lock();
        let doc = Doc::new();
        comments::write(&doc, &comment("c1", "c1", "what do you think?", "u1", 10));
        comments::write(&doc, &comment("c2", "c1", "I agree", "u2", 20));
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        // Alice started the thread, so Bob's reply notifies her.
        assert_eq!(
            notifications(&conn, "u1"),
            vec![("c2".into(), "reply".into())]
        );
        // The comment that started the thread does not notify the replier.
        assert!(notifications(&conn, "u2").is_empty());

        // Running again changes nothing.
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        assert_eq!(notifications(&conn, "u1").len(), 1);
    }

    #[test]
    fn resolving_a_comment_clears_its_notifications() {
        let e = env();
        let conn = e.db.lock();
        let doc = Doc::new();
        comments::write(
            &doc,
            &comment("c1", "c1", "hi @[Bob](kawruh://user/u2)", "u1", 10),
        );
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        assert_eq!(notifications(&conn, "u2").len(), 1);

        let mut root = comment("c1", "c1", "hi @[Bob](kawruh://user/u2)", "u1", 10);
        root.resolved = true;
        comments::write(&doc, &root);
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        assert!(notifications(&conn, "u2").is_empty());
    }

    #[test]
    fn a_deleted_comment_clears_its_notifications() {
        let e = env();
        let conn = e.db.lock();
        let doc = Doc::new();
        comments::write(
            &doc,
            &comment("c1", "c1", "hi @[Bob](kawruh://user/u2)", "u1", 10),
        );
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        assert_eq!(notifications(&conn, "u2").len(), 1);
        comments::remove(&doc, "c1");
        sync_page_notifications(&conn, "w1", "p1", &doc).unwrap();
        assert!(notifications(&conn, "u2").is_empty());
    }
}
