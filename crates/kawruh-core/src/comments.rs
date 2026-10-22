//! Comments on a page: threads of notes, either about the page or about a
//! piece of its text.
//!
//! Layout (mirrored by `app/src/lib/comments.ts`):
//! `comments: Map<commentId, JSON string>` in the page's Y.Doc.
//!
//! Every comment is one entry. A reply is another entry that carries the same
//! `thread` (the id of the root comment) and the id of the comment it answers
//! in `reply_to`. Two people writing in the same thread at the same time
//! therefore never overwrite each other.
//!
//! A comment that is about a piece of text carries the quoted text in `quote`
//! and the two ends of it in `anchor` and `anchor_end`, as encoded Yjs
//! relative positions. Only the app resolves those; the server keeps them as
//! they are.
//!
//! Mentions are ordinary markdown links to a user:
//! `@[Name](kawruh://user/<user_id>)`.

use serde::{Deserialize, Serialize};
use yrs::{Any, Doc, Map, MapRef, Out, Transact};

pub const COMMENTS: &str = "comments";
/// The link prefix that mentions a user.
pub const USER_LINK: &str = "kawruh://user/";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Comment {
    pub id: String,
    /// The root comment of the thread; a root comment carries its own id.
    #[serde(default)]
    pub thread: String,
    /// The comment this one answers; empty for a root comment.
    #[serde(default)]
    pub reply_to: String,
    /// The text the comment is about; empty for a comment on the page itself.
    #[serde(default)]
    pub quote: String,
    /// Where the quoted text starts, as an encoded Yjs relative position.
    #[serde(default)]
    pub anchor: String,
    /// Where the quoted text ends, as an encoded Yjs relative position.
    #[serde(default)]
    pub anchor_end: String,
    /// The comment itself, as markdown.
    pub body: String,
    /// The author's user id.
    #[serde(default)]
    pub author: String,
    /// The author's display name, so the comment reads well without a lookup.
    #[serde(default)]
    pub author_name: String,
    /// When the comment was written (ms since the epoch).
    pub created: i64,
    #[serde(default)]
    pub resolved: bool,
}

impl Comment {
    /// A new root comment on a page.
    pub fn new(id: &str, body: &str, author: &str, author_name: &str, created: i64) -> Comment {
        Comment {
            id: id.to_string(),
            thread: id.to_string(),
            reply_to: String::new(),
            quote: String::new(),
            anchor: String::new(),
            anchor_end: String::new(),
            body: body.to_string(),
            author: author.to_string(),
            author_name: author_name.to_string(),
            created,
            resolved: false,
        }
    }

    /// A reply in the thread of `root`.
    pub fn reply(
        id: &str,
        root: &str,
        reply_to: &str,
        body: &str,
        author: &str,
        author_name: &str,
        created: i64,
    ) -> Comment {
        Comment {
            id: id.to_string(),
            thread: root.to_string(),
            reply_to: reply_to.to_string(),
            quote: String::new(),
            anchor: String::new(),
            anchor_end: String::new(),
            body: body.to_string(),
            author: author.to_string(),
            author_name: author_name.to_string(),
            created,
            resolved: false,
        }
    }

    /// The users mentioned in the body, in the order they appear.
    pub fn mentions(&self) -> Vec<String> {
        mentioned_users(&self.body)
    }

    /// Whether the comment is about a piece of text.
    pub fn is_anchored(&self) -> bool {
        !self.anchor.is_empty() && !self.anchor_end.is_empty()
    }
}

/// The user ids mentioned in `body`.
pub fn mentioned_users(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in body.match_indices(USER_LINK) {
        let rest = &body[i + USER_LINK.len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_')
            .unwrap_or(rest.len());
        if end == 0 {
            continue;
        }
        let id = &rest[..end];
        if !out.iter().any(|u| u == id) {
            out.push(id.to_string());
        }
    }
    out
}

/// The comment map of a page document.
pub fn map(doc: &Doc) -> MapRef {
    doc.get_or_insert_map(COMMENTS)
}

/// Every comment of a page, oldest first.
pub fn read(doc: &Doc) -> Vec<Comment> {
    let comments = map(doc);
    let txn = doc.transact();
    let mut out: Vec<Comment> = comments
        .iter(&txn)
        .filter_map(|(_, v)| match v {
            Out::Any(Any::String(s)) => serde_json::from_str::<Comment>(&s).ok(),
            _ => None,
        })
        .collect();
    out.sort_by(|a, b| a.created.cmp(&b.created).then_with(|| a.id.cmp(&b.id)));
    out
}

/// The comments of a page, grouped into threads (root comment first, replies
/// in order).
pub fn threads(doc: &Doc) -> Vec<Vec<Comment>> {
    let all = read(doc);
    let mut roots: Vec<Comment> = Vec::new();
    let mut replies: Vec<Comment> = Vec::new();
    for c in all {
        if c.thread.is_empty() || c.thread == c.id {
            roots.push(c);
        } else {
            replies.push(c);
        }
    }
    roots
        .into_iter()
        .map(|root| {
            let mut thread = vec![root.clone()];
            for r in replies.iter().filter(|r| r.thread == root.id) {
                thread.push(r.clone());
            }
            thread
        })
        .collect()
}

/// Writes a comment, replacing the one with the same id.
pub fn write(doc: &Doc, comment: &Comment) {
    let comments = map(doc);
    let json = serde_json::to_string(comment).unwrap_or_default();
    let mut txn = doc.transact_mut();
    comments.insert(&mut txn, comment.id.as_str(), json);
}

/// Removes a comment.
pub fn remove(doc: &Doc, id: &str) {
    let comments = map(doc);
    let mut txn = doc.transact_mut();
    comments.remove(&mut txn, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(comments: &[Comment]) -> Doc {
        let doc = Doc::new();
        for c in comments {
            write(&doc, c);
        }
        doc
    }

    #[test]
    fn comments_round_trip_through_the_map() {
        let root = Comment::new("c1", "Hello", "u1", "Alice", 10);
        let reply = Comment::reply("c2", "c1", "c1", "Hi", "u2", "Bob", 20);
        let doc = doc_with(&[root.clone(), reply.clone()]);
        let all = read(&doc);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], root);
        assert_eq!(all[1], reply);
        // A root comment is its own thread.
        assert_eq!(all[0].thread, all[0].id);
    }

    #[test]
    fn threads_keep_the_root_and_its_replies_together() {
        let root = Comment::new("c1", "Hello", "u1", "Alice", 10);
        let reply = Comment::reply("c2", "c1", "c1", "Hi", "u2", "Bob", 20);
        let other = Comment::new("c3", "Another", "u1", "Alice", 30);
        let doc = doc_with(&[root, reply, other]);
        let threads = threads(&doc);
        assert_eq!(threads.len(), 2);
        assert_eq!(threads[0].len(), 2);
        assert_eq!(threads[0][0].id, "c1");
        assert_eq!(threads[0][1].id, "c2");
        assert_eq!(threads[1].len(), 1);
        assert_eq!(threads[1][0].id, "c3");
    }

    #[test]
    fn mentions_are_read_from_the_body() {
        let mut c = Comment::new(
            "c1",
            "hi @[Alice](kawruh://user/u1) and @[Bob](kawruh://user/u2)",
            "u1",
            "Alice",
            1,
        );
        assert_eq!(c.mentions(), vec!["u1", "u2"]);
        // The same user twice is mentioned once.
        c.body = "@[Alice](kawruh://user/u1) @[Alice again](kawruh://user/u1)".into();
        assert_eq!(c.mentions(), vec!["u1"]);
        // A longer id that starts with another one is a different user.
        c.body = "@[A](kawruh://user/u1) @[B](kawruh://user/u12)".into();
        assert_eq!(c.mentions(), vec!["u1", "u12"]);
        // Nothing to mention.
        c.body = "no mentions here".into();
        assert!(c.mentions().is_empty());
    }

    #[test]
    fn writing_the_same_id_replaces_the_comment() {
        let doc = Doc::new();
        let mut c = Comment::new("c1", "First", "u1", "Alice", 10);
        write(&doc, &c);
        c.body = "Second".into();
        c.resolved = true;
        write(&doc, &c);
        let all = read(&doc);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].body, "Second");
        assert!(all[0].resolved);
        remove(&doc, "c1");
        assert!(read(&doc).is_empty());
    }

    #[test]
    fn broken_entries_are_skipped() {
        let doc = Doc::new();
        {
            let comments = map(&doc);
            let mut txn = doc.transact_mut();
            comments.insert(&mut txn, "bad", "not json");
            comments.insert(&mut txn, "good", r#"{"id":"g","body":"hi","created":1}"#);
        }
        let all = read(&doc);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "g");
    }
}
