//! Disk cleanup: image assets no document refers to any more, and the
//! documents of pages purged from the trash.
//!
//! An asset counts as referenced while its SHA-256 (the file stem) occurs in
//! the raw bytes of any stored document snapshot or pending update, of any
//! workspace. That is deliberately blunt: it does not depend on the derived
//! `page_text` index, it keeps images of trashed pages and of updates not yet
//! compacted, and a false match only ever keeps a file. An unreferenced asset
//! is recorded in `orphan_assets` and deleted once it has stayed unreferenced
//! for `GRACE_MS`; temp files of abandoned uploads go after a day.
//!
//! A page document whose page is gone from its workspace's page tree (purged,
//! not just trashed), unchanged for `GRACE_MS` and not loaded is deleted with
//! its search-index row and share links.
//!
//! The server sweeps 10 minutes after start and then daily;
//! `kawruh-server admin gc [--dry-run]` sweeps on demand.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::Duration;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use yrs::{Doc, Map, Transact};

use kawruh_core::store;
use kawruh_core::workspace::{self, now_ms};

use crate::assets;
use crate::db::Db;
use crate::docs::DocRegistry;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// How long an asset stays unreferenced, or a purged page's document
/// unchanged, before it is deleted.
pub const GRACE_MS: i64 = 7 * DAY_MS;
/// Temp files of uploads older than this were abandoned.
const TEMP_MAX_AGE_MS: i64 = DAY_MS;

const FIRST_SWEEP: Duration = Duration::from_secs(10 * 60);
const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetState {
    /// Some document refers to it.
    Referenced,
    /// Found unreferenced by this sweep.
    Marked,
    /// Unreferenced since `since`, not for long enough to delete.
    Pending { since: i64 },
    /// Deleted by this sweep (in a dry run: would be).
    Deleted,
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub size: u64,
    pub state: AssetState,
}

#[derive(Debug, Default)]
pub struct WorkspaceAssets {
    pub workspace_id: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub dry_run: bool,
    pub now: i64,
    pub workspaces: Vec<WorkspaceAssets>,
    /// Abandoned upload temp files removed, and their bytes.
    pub temp_removed: usize,
    pub temp_bytes: u64,
    /// (workspace, page) of purged pages' documents deleted.
    pub docs_deleted: Vec<(String, String)>,
    /// Workspaces whose page documents were left alone, and why.
    pub skipped: Vec<(String, String)>,
}

impl Report {
    fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.workspaces.iter().flat_map(|w| &w.assets)
    }

    fn count(&self, f: impl Fn(&AssetState) -> bool) -> usize {
        self.assets().filter(|a| f(&a.state)).count()
    }

    pub fn assets_deleted(&self) -> usize {
        self.count(|s| *s == AssetState::Deleted)
    }

    /// Unreferenced assets kept for now (including the ones just marked).
    pub fn orphans_pending(&self) -> usize {
        self.count(|s| matches!(s, AssetState::Marked | AssetState::Pending { .. }))
    }

    pub fn bytes_freed(&self) -> u64 {
        self.assets()
            .filter(|a| a.state == AssetState::Deleted)
            .map(|a| a.size)
            .sum::<u64>()
            + self.temp_bytes
    }

    /// One line for the log.
    pub fn summary(&self) -> String {
        format!(
            "{} asset(s) deleted, {} freed, {} orphan(s) pending, {} abandoned upload(s) removed, \
             {} purged page document(s) deleted",
            self.assets_deleted(),
            human_bytes(self.bytes_freed()),
            self.orphans_pending(),
            self.temp_removed,
            self.docs_deleted.len()
        )
    }

    /// The per-workspace report printed by `admin gc`.
    pub fn render(&self) -> String {
        let (deleted, marked) = if self.dry_run {
            ("would delete", "would mark")
        } else {
            ("deleted", "marked")
        };
        let mut out = String::new();
        for ws in &self.workspaces {
            let referenced = ws
                .assets
                .iter()
                .filter(|a| a.state == AssetState::Referenced)
                .count();
            let _ = writeln!(
                out,
                "workspace {}: {} asset(s), {referenced} referenced",
                ws.workspace_id,
                ws.assets.len()
            );
            for a in &ws.assets {
                let what = match a.state {
                    AssetState::Referenced => continue,
                    AssetState::Marked => format!("{marked:<12}  newly unreferenced"),
                    AssetState::Pending { since } => format!(
                        "{:<12}  unreferenced for {}, deleted after {}",
                        "pending",
                        age(self.now - since),
                        age(GRACE_MS)
                    ),
                    AssetState::Deleted => {
                        format!("{deleted:<12}  unreferenced for over {}", age(GRACE_MS))
                    }
                };
                let _ = writeln!(out, "  {what}  {}  {}", a.name, human_bytes(a.size));
            }
        }
        for (ws, why) in &self.skipped {
            let _ = writeln!(out, "workspace {ws}: page documents skipped ({why})");
        }
        for (ws, page) in &self.docs_deleted {
            let _ = writeln!(
                out,
                "{deleted} document of purged page {page} (workspace {ws})"
            );
        }
        let _ = writeln!(
            out,
            "assets: {} {deleted}, {} pending; abandoned uploads: {} {deleted}; {} {}",
            self.assets_deleted(),
            self.orphans_pending(),
            self.temp_removed,
            human_bytes(self.bytes_freed()),
            if self.dry_run { "to free" } else { "freed" }
        );
        let _ = writeln!(
            out,
            "purged page documents: {} {deleted}",
            self.docs_deleted.len()
        );
        if self.dry_run {
            out.push_str("dry run: nothing was changed\n");
        }
        out
    }
}

fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / (1 << 10) as f64),
        n => format!("{n} B"),
    }
}

fn age(ms: i64) -> String {
    if ms >= DAY_MS {
        format!("{}d", ms / DAY_MS)
    } else {
        format!("{}h", ms.max(0) / (60 * 60 * 1000))
    }
}

/// Sweeps every `SWEEP_EVERY`, the first time `FIRST_SWEEP` after start.
pub async fn run_periodically(reg: Weak<DocRegistry>) {
    let start = tokio::time::Instant::now() + FIRST_SWEEP;
    let mut tick = tokio::time::interval_at(start, SWEEP_EVERY);
    loop {
        tick.tick().await;
        let Some(reg) = reg.upgrade() else { return };
        let result = tokio::task::spawn_blocking(move || {
            sweep(&reg.db, &reg.data_dir, Some(&reg), now_ms(), false)
        })
        .await;
        match result {
            Ok(Ok(report)) => tracing::info!("cleanup: {}", report.summary()),
            Ok(Err(e)) => tracing::warn!("cleanup failed: {e}"),
            Err(e) => tracing::warn!("cleanup task failed: {e}"),
        }
    }
}

/// One cleanup pass at time `now` (ms). `live` is the running server's
/// registry, whose loaded documents are never deleted; `admin gc` runs without
/// one and relies on the grace period alone. A dry run changes nothing and
/// reports what a real sweep would do.
pub fn sweep(
    db: &Db,
    data_dir: &Path,
    live: Option<&DocRegistry>,
    now: i64,
    dry_run: bool,
) -> anyhow::Result<Report> {
    let mut report = Report {
        dry_run,
        now,
        ..Report::default()
    };
    // Blobs are scanned through a connection of our own, so the server's
    // shared one stays free for live edits meanwhile.
    let path = db
        .lock()
        .path()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("the database has no file to scan"))?;
    let reader = Connection::open(path)?;
    sweep_assets(db, &reader, data_dir, now, &mut report)?;
    sweep_docs(db, &reader, live, now, &mut report)?;
    Ok(report)
}

struct Stored {
    workspace_id: String,
    name: String,
    size: u64,
}

impl Stored {
    fn hash(&self) -> &str {
        self.name.split_once('.').map_or("", |(h, _)| h)
    }
}

/// Stored assets of all workspaces. Removes abandoned upload temp files.
fn list_assets(data_dir: &Path, now: i64, report: &mut Report) -> std::io::Result<Vec<Stored>> {
    let root = data_dir.join("assets");
    let dirs = match std::fs::read_dir(&root) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for dir in dirs {
        let dir = dir?;
        let workspace_id = dir.file_name().to_string_lossy().to_string();
        if !crate::api::valid_id(&workspace_id) || !dir.file_type()?.is_dir() {
            continue;
        }
        for file in std::fs::read_dir(dir.path())? {
            let file = file?;
            let name = file.file_name().to_string_lossy().to_string();
            let Ok(meta) = file.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            if assets::parse_name(&name).is_some() {
                out.push(Stored {
                    workspace_id: workspace_id.clone(),
                    name,
                    size: meta.len(),
                });
            } else if name.starts_with('.') && name.ends_with(".tmp") {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(now, |d| d.as_millis() as i64);
                if now - modified > TEMP_MAX_AGE_MS
                    && (report.dry_run || std::fs::remove_file(file.path()).is_ok())
                {
                    report.temp_removed += 1;
                    report.temp_bytes += meta.len();
                }
            }
        }
    }
    out.sort_by(|a, b| (&a.workspace_id, &a.name).cmp(&(&b.workspace_id, &b.name)));
    Ok(out)
}

fn is_hex(b: u8) -> bool {
    matches!(b, b'0'..=b'9' | b'a'..=b'f')
}

/// Adds to `found` each of `hashes` that occurs in `blob`.
fn find_hashes(blob: &[u8], hashes: &HashSet<&str>, found: &mut HashSet<String>) {
    let mut i = 0;
    while i < blob.len() {
        if !is_hex(blob[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < blob.len() && is_hex(blob[i]) {
            i += 1;
        }
        if i - start >= 64 {
            for window in blob[start..i].windows(64) {
                if let Ok(s) = std::str::from_utf8(window)
                    && hashes.contains(s)
                {
                    found.insert(s.to_string());
                }
            }
        }
    }
}

fn scan_rows(
    conn: &Connection,
    sql: &str,
    param: i64,
    hashes: &HashSet<&str>,
    found: &mut HashSet<String>,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([param])?;
    while let Some(row) = rows.next()? {
        if let ValueRef::Blob(blob) = row.get_ref(0)? {
            find_hashes(blob, hashes, found);
        }
    }
    Ok(())
}

/// Which of `hashes` some stored document mentions, from one consistent
/// snapshot (so compaction moving updates into a snapshot can't hide one),
/// and the last update sequence number that snapshot includes.
fn referenced(
    conn: &Connection,
    hashes: &HashSet<&str>,
) -> rusqlite::Result<(HashSet<String>, i64)> {
    let tx = conn.unchecked_transaction()?;
    let last_seq: i64 = tx.query_row("SELECT COALESCE(MAX(seq), 0) FROM doc_updates", [], |r| {
        r.get(0)
    })?;
    let mut found = HashSet::new();
    scan_rows(
        &tx,
        "SELECT state FROM docs WHERE state IS NOT NULL AND ?1 = ?1",
        0,
        hashes,
        &mut found,
    )?;
    scan_rows(
        &tx,
        "SELECT data FROM doc_updates WHERE seq <= ?1",
        last_seq,
        hashes,
        &mut found,
    )?;
    Ok((found, last_seq))
}

/// Which of `hashes` documents written after the scan mention: updates past
/// `last_seq`, and snapshots compacted since `scan_started`.
fn referenced_since(
    conn: &Connection,
    hashes: &HashSet<&str>,
    last_seq: i64,
    scan_started: i64,
) -> rusqlite::Result<HashSet<String>> {
    let mut found = HashSet::new();
    scan_rows(
        conn,
        "SELECT data FROM doc_updates WHERE seq > ?1",
        last_seq,
        hashes,
        &mut found,
    )?;
    scan_rows(
        conn,
        "SELECT state FROM docs WHERE state IS NOT NULL AND updated_at >= ?1",
        scan_started,
        hashes,
        &mut found,
    )?;
    Ok(found)
}

fn sweep_assets(
    db: &Db,
    reader: &Connection,
    data_dir: &Path,
    now: i64,
    report: &mut Report,
) -> anyhow::Result<()> {
    let stored = list_assets(data_dir, now, report)?;
    let hashes: HashSet<&str> = stored.iter().map(Stored::hash).collect();
    let scan_started = now_ms();
    let (mut found, last_seq) = if hashes.is_empty() {
        (HashSet::new(), 0)
    } else {
        referenced(reader, &hashes)?
    };

    // Everything below is quick and runs under the write lock, so a document
    // update can't land between the last check and a deletion.
    let mut conn = db.lock();
    let behavior = if report.dry_run {
        TransactionBehavior::Deferred
    } else {
        TransactionBehavior::Immediate
    };
    let tx = conn.transaction_with_behavior(behavior)?;
    let mut orphans: HashMap<(String, String), i64> = {
        let mut stmt = tx.prepare("SELECT workspace_id, name, since FROM orphan_assets")?;
        let rows = stmt.query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let due: HashSet<&str> = stored
        .iter()
        .filter(|s| !found.contains(s.hash()))
        .filter(|s| {
            orphans
                .get(&(s.workspace_id.clone(), s.name.clone()))
                .is_some_and(|since| now - since >= GRACE_MS)
        })
        .map(Stored::hash)
        .collect();
    if !due.is_empty() {
        found.extend(referenced_since(&tx, &due, last_seq, scan_started)?);
    }

    let mut by_workspace: Vec<WorkspaceAssets> = Vec::new();
    for s in &stored {
        let key = (s.workspace_id.clone(), s.name.clone());
        let since = orphans.remove(&key);
        let state = match since {
            _ if found.contains(s.hash()) => {
                if since.is_some() && !report.dry_run {
                    tx.execute(
                        "DELETE FROM orphan_assets WHERE workspace_id = ?1 AND name = ?2",
                        params![key.0, key.1],
                    )?;
                }
                AssetState::Referenced
            }
            None => {
                if !report.dry_run {
                    tx.execute(
                        "INSERT INTO orphan_assets (workspace_id, name, since) VALUES (?1, ?2, ?3)",
                        params![key.0, key.1, now],
                    )?;
                }
                AssetState::Marked
            }
            Some(since) if now - since < GRACE_MS => AssetState::Pending { since },
            Some(since) => {
                let path = assets::workspace_dir(data_dir, &s.workspace_id).join(&s.name);
                if report.dry_run {
                    AssetState::Deleted
                } else {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            tx.execute(
                                "DELETE FROM orphan_assets WHERE workspace_id = ?1 AND name = ?2",
                                params![key.0, key.1],
                            )?;
                            AssetState::Deleted
                        }
                        Err(e) => {
                            tracing::warn!("deleting {}: {e}", path.display());
                            AssetState::Pending { since }
                        }
                    }
                }
            }
        };
        if by_workspace
            .last()
            .is_none_or(|w| w.workspace_id != s.workspace_id)
        {
            by_workspace.push(WorkspaceAssets {
                workspace_id: s.workspace_id.clone(),
                assets: Vec::new(),
            });
        }
        if let Some(ws) = by_workspace.last_mut() {
            ws.assets.push(Asset {
                name: s.name.clone(),
                size: s.size,
                state,
            });
        }
    }
    // Rows left over are for files that are gone (workspace deleted, …).
    if !report.dry_run {
        for (ws, name) in orphans.keys() {
            tx.execute(
                "DELETE FROM orphan_assets WHERE workspace_id = ?1 AND name = ?2",
                params![ws, name],
            )?;
        }
        tx.commit()?;
    }
    report.workspaces = by_workspace;
    Ok(())
}

/// Ids in the workspace document's page tree, trashed pages included.
fn page_ids(doc: &Doc) -> HashSet<String> {
    let pages = doc.get_or_insert_map(workspace::PAGES);
    let txn = doc.transact();
    pages.keys(&txn).map(str::to_string).collect()
}

/// Deletes a purged page's document, its index row and share links, unless it
/// was written to since `cutoff`. Returns whether it did.
fn delete_page_doc(
    db: &Db,
    workspace_id: &str,
    page_id: &str,
    cutoff: i64,
) -> rusqlite::Result<bool> {
    let mut conn = db.lock();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let updated_at: Option<i64> = tx
        .query_row(
            "SELECT updated_at FROM docs WHERE id = ?1 AND workspace_id = ?2 AND kind = 'page'",
            [page_id, workspace_id],
            |r| r.get(0),
        )
        .optional()?;
    if updated_at.is_none_or(|t| t > cutoff) {
        return Ok(false);
    }
    tx.execute("DELETE FROM doc_updates WHERE doc_id = ?1", [page_id])?;
    tx.execute("DELETE FROM docs WHERE id = ?1", [page_id])?;
    tx.execute("DELETE FROM page_text WHERE page_id = ?1", [page_id])?;
    tx.execute(
        "DELETE FROM shares WHERE page_id = ?1 AND workspace_id = ?2",
        [page_id, workspace_id],
    )?;
    tx.commit()?;
    Ok(true)
}

fn sweep_docs(
    db: &Db,
    reader: &Connection,
    live: Option<&DocRegistry>,
    now: i64,
    report: &mut Report,
) -> anyhow::Result<()> {
    // `docs.updated_at` is bumped by every appended update and compaction.
    let cutoff = now - GRACE_MS;
    let mut stale: HashMap<String, Vec<String>> = HashMap::new();
    {
        let mut stmt = reader.prepare(
            "SELECT workspace_id, id FROM docs
             WHERE kind = 'page' AND id != workspace_id AND updated_at <= ?1
             ORDER BY workspace_id, id",
        )?;
        let rows = stmt.query_map([cutoff], |r| Ok((r.get::<_, String>(0)?, r.get(1)?)))?;
        for row in rows {
            let (ws, id) = row?;
            stale.entry(ws).or_default().push(id);
        }
    }
    let mut workspaces: Vec<_> = stale.into_iter().collect();
    workspaces.sort();
    for (ws, candidates) in workspaces {
        let ids = match live.and_then(|reg| reg.loaded(&ws)) {
            Some(doc) => Ok(Some(page_ids(&doc.doc))),
            None => store::load_doc(reader, &ws).map(|d| d.as_ref().map(page_ids)),
        };
        let ids = match ids {
            Ok(Some(ids)) if !ids.is_empty() => ids,
            Ok(Some(_)) => {
                report.skipped.push((ws, "its page tree is empty".into()));
                continue;
            }
            Ok(None) => {
                report.skipped.push((ws, "no workspace document".into()));
                continue;
            }
            Err(e) => {
                report
                    .skipped
                    .push((ws, format!("workspace document unreadable: {e}")));
                continue;
            }
        };
        for page in candidates.into_iter().filter(|p| !ids.contains(p)) {
            let deleted = if report.dry_run {
                live.is_none_or(|reg| reg.loaded(&page).is_none())
            } else {
                let run = || delete_page_doc(db, &ws, &page, cutoff);
                match live {
                    Some(reg) => reg.unless_loaded(&page, run).transpose()?.unwrap_or(false),
                    None => run()?,
                }
            };
            if deleted {
                report.docs_deleted.push((ws.clone(), page));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kawruh_core::markdown;
    use kawruh_core::store::DocKind;
    use yrs::{ReadTxn, StateVector};

    struct Env {
        dir: tempfile::TempDir,
        db: Db,
    }

    fn env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("kawruh.db")).unwrap();
        Env { dir, db }
    }

    fn full_update(doc: &Doc) -> Vec<u8> {
        doc.transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    impl Env {
        /// Stores an image file; returns its asset name.
        fn asset(&self, ws: &str, content: &str) -> String {
            let name = format!(
                "{}.png",
                hex::encode(<sha2::Sha256 as sha2::Digest>::digest(content))
            );
            let dir = assets::workspace_dir(self.dir.path(), ws);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(&name), content).unwrap();
            name
        }

        fn has_asset(&self, ws: &str, name: &str) -> bool {
            assets::workspace_dir(self.dir.path(), ws)
                .join(name)
                .exists()
        }

        /// A page document showing `images`, as one pending update or (when
        /// `compacted`) as the snapshot.
        fn page(&self, ws: &str, page: &str, images: &[&str], compacted: bool) {
            let doc = Doc::new();
            let md: String = images
                .iter()
                .map(|n| format!("Text\n\n![x](kawruh-asset://{ws}/{n})\n\n"))
                .collect();
            markdown::set_markdown(&doc, &format!("Hello\n\n{md}"), "t");
            let mut conn = self.db.lock();
            store::ensure_doc(&conn, page, ws, DocKind::Page).unwrap();
            store::append_update(&conn, page, &full_update(&doc)).unwrap();
            if compacted {
                store::compact(&mut conn, page, &doc).unwrap();
            }
        }

        /// The workspace document, with `pages` in its tree (`true` = trashed).
        fn workspace(&self, ws: &str, pages: &[(&str, bool)]) {
            let doc = Doc::new();
            workspace::set_workspace_name(&doc, "W", "t");
            for (id, trashed) in pages {
                workspace::create_page_with_id(&doc, id, id, "", "t").unwrap();
                if *trashed {
                    workspace::delete_page(&doc, id, "t").unwrap();
                }
            }
            let conn = self.db.lock();
            store::ensure_doc(&conn, ws, ws, DocKind::Workspace).unwrap();
            store::append_update(&conn, ws, &full_update(&doc)).unwrap();
        }

        fn sweep(&self, now: i64, dry_run: bool) -> Report {
            sweep(&self.db, self.dir.path(), None, now, dry_run).unwrap()
        }

        fn orphan_rows(&self) -> Vec<String> {
            let conn = self.db.lock();
            let mut stmt = conn
                .prepare("SELECT name FROM orphan_assets ORDER BY name")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        }

        fn count(&self, sql: &str, id: &str) -> i64 {
            self.db.lock().query_row(sql, [id], |r| r.get(0)).unwrap()
        }
    }

    fn state_of(report: &Report, name: &str) -> Option<AssetState> {
        report.assets().find(|a| a.name == name).map(|a| a.state)
    }

    #[test]
    fn finds_hashes_inside_longer_hex_runs() {
        let h = "a".repeat(63) + "b";
        let hashes = HashSet::from([h.as_str()]);
        let mut found = HashSet::new();
        find_hashes(format!("xx{h}.png").as_bytes(), &hashes, &mut found);
        assert_eq!(found.len(), 1);
        found.clear();
        find_hashes(format!("ff{h}0").as_bytes(), &hashes, &mut found);
        assert_eq!(found.len(), 1);
        found.clear();
        find_hashes(&h.as_bytes()[..63], &hashes, &mut found);
        assert!(found.is_empty());
    }

    #[test]
    fn referenced_assets_stay_and_orphans_go_after_the_grace_period() {
        let e = env();
        let live = e.asset("w1", "live");
        let trashed = e.asset("w1", "trashed");
        let pending = e.asset("w1", "pending");
        let foreign = e.asset("w1", "foreign");
        let orphan = e.asset("w1", "orphan");
        e.workspace("w1", &[("p1", false), ("p2", true), ("p3", false)]);
        e.page("w1", "p1", &[&live], true);
        e.page("w1", "p2", &[&trashed], true);
        // Not compacted yet: only in `doc_updates`.
        e.page("w1", "p3", &[&pending], false);
        // A page of another workspace pointing here keeps it too.
        e.workspace("w2", &[("q1", false)]);
        e.page("w2", "q1", &[&foreign], true);

        let t0 = now_ms();
        let r = e.sweep(t0, false);
        for name in [&live, &trashed, &pending, &foreign] {
            assert_eq!(state_of(&r, name), Some(AssetState::Referenced), "{name}");
        }
        assert_eq!(state_of(&r, &orphan), Some(AssetState::Marked));
        assert_eq!(e.orphan_rows(), vec![orphan.clone()]);
        assert_eq!((r.assets_deleted(), r.orphans_pending()), (0, 1));

        let r = e.sweep(t0 + GRACE_MS - 1, false);
        assert_eq!(
            state_of(&r, &orphan),
            Some(AssetState::Pending { since: t0 })
        );
        assert!(e.has_asset("w1", &orphan));

        let r = e.sweep(t0 + GRACE_MS, false);
        assert_eq!(state_of(&r, &orphan), Some(AssetState::Deleted));
        assert_eq!(r.bytes_freed(), "orphan".len() as u64);
        assert!(!e.has_asset("w1", &orphan));
        assert!(e.orphan_rows().is_empty());
        for name in [&live, &trashed, &pending, &foreign] {
            assert!(e.has_asset("w1", name), "{name}");
        }
        assert!(
            r.render()
                .contains(&format!("deleted       unreferenced for over 7d  {orphan}"))
        );
    }

    #[test]
    fn an_orphan_used_again_is_unmarked() {
        let e = env();
        let img = e.asset("w1", "img");
        e.workspace("w1", &[("p1", false)]);
        e.page("w1", "p1", &[], true);
        let t0 = now_ms();
        e.sweep(t0, false);
        assert_eq!(e.orphan_rows(), vec![img.clone()]);

        e.page("w1", "p1", &[&img], false);
        let r = e.sweep(t0 + GRACE_MS, false);
        assert_eq!(state_of(&r, &img), Some(AssetState::Referenced));
        assert!(e.orphan_rows().is_empty());
        assert!(e.has_asset("w1", &img));
    }

    #[test]
    fn dry_runs_change_nothing() {
        let e = env();
        let img = e.asset("w1", "img");
        let tmp = assets::workspace_dir(e.dir.path(), "w1").join(".x.png.1.tmp");
        std::fs::write(&tmp, b"partial").unwrap();
        e.workspace("w1", &[("p1", false)]);
        e.page("w1", "p1", &[], true);
        e.page("w1", "gone", &[], true);

        let t0 = now_ms();
        let r = e.sweep(t0, true);
        assert_eq!(state_of(&r, &img), Some(AssetState::Marked));
        assert!(e.orphan_rows().is_empty());

        e.sweep(t0, false);
        let later = t0 + GRACE_MS + DAY_MS;
        let r = e.sweep(later, true);
        assert_eq!(state_of(&r, &img), Some(AssetState::Deleted));
        assert_eq!(r.temp_removed, 1);
        assert_eq!(r.docs_deleted, vec![("w1".into(), "gone".into())]);
        assert!(r.render().contains("dry run: nothing was changed"));
        assert!(e.has_asset("w1", &img));
        assert!(tmp.exists());
        assert_eq!(e.orphan_rows(), vec![img.clone()]);
        assert_eq!(
            e.count("SELECT COUNT(*) FROM docs WHERE id = ?1", "gone"),
            1
        );
    }

    #[test]
    fn abandoned_uploads_and_stale_rows_are_cleared() {
        let e = env();
        let dir = assets::workspace_dir(e.dir.path(), "w1");
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = dir.join(".x.png.1.tmp");
        std::fs::write(&tmp, b"partial").unwrap();
        e.db.lock()
            .execute(
                "INSERT INTO orphan_assets (workspace_id, name, since) VALUES ('w9', 'gone.png', 1)",
                [],
            )
            .unwrap();
        let t0 = now_ms();
        let r = e.sweep(t0, false);
        assert_eq!(r.temp_removed, 0);
        assert!(tmp.exists());
        assert!(e.orphan_rows().is_empty());

        let r = e.sweep(t0 + DAY_MS + 60_000, false);
        assert_eq!((r.temp_removed, r.temp_bytes), (1, 7));
        assert!(!tmp.exists());
    }

    #[test]
    fn purged_pages_lose_their_documents_after_the_grace_period() {
        let e = env();
        e.workspace("w1", &[("live", false), ("trashed", true)]);
        for p in ["live", "trashed", "purged", "recent"] {
            e.page("w1", p, &[], p == "live");
        }
        {
            let conn = e.db.lock();
            for p in ["purged", "recent"] {
                db_rows(&conn, "w1", p);
            }
        }
        let now = now_ms() + GRACE_MS + 1000;
        // Written to within the grace period.
        e.db.lock()
            .execute(
                "UPDATE docs SET updated_at = ?1 WHERE id = 'recent'",
                [now - GRACE_MS + 1],
            )
            .unwrap();

        let r = e.sweep(now, false);
        assert_eq!(r.docs_deleted, vec![("w1".into(), "purged".into())]);
        for (sql, n) in [
            ("SELECT COUNT(*) FROM docs WHERE id = ?1", 0),
            ("SELECT COUNT(*) FROM doc_updates WHERE doc_id = ?1", 0),
            ("SELECT COUNT(*) FROM page_text WHERE page_id = ?1", 0),
            ("SELECT COUNT(*) FROM shares WHERE page_id = ?1", 0),
        ] {
            assert_eq!(e.count(sql, "purged"), n, "{sql}");
            assert_eq!(e.count(sql, "recent"), 1, "{sql}");
        }
        for p in ["live", "trashed", "w1"] {
            assert_eq!(e.count("SELECT COUNT(*) FROM docs WHERE id = ?1", p), 1);
        }
    }

    /// Index row and share link of a page.
    fn db_rows(conn: &Connection, ws: &str, page: &str) {
        crate::db::upsert_page_text(conn, page, ws, Some("T"), Some("md"), Some(true)).unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO users (id, username, display_name, password_hash, created_at) VALUES ('u1','alice','Alice','x',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO workspaces (id, name, owner_id, created_at) VALUES (?1,'W','u1',1)",
            [ws],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO shares (token, workspace_id, page_id, created_by, created_at) VALUES (?1, ?2, ?3, 'u1', 1)",
            params![format!("tok-{page}"), ws, page],
        )
        .unwrap();
    }

    #[test]
    fn workspaces_without_a_page_tree_are_skipped() {
        let e = env();
        // A workspace document with a name but no pages.
        e.workspace("w1", &[]);
        e.page("w1", "p1", &[], true);
        // Page documents but no workspace document at all.
        e.page("w2", "q1", &[], true);
        let r = e.sweep(now_ms() + GRACE_MS + 1000, false);
        assert!(r.docs_deleted.is_empty());
        let skipped: Vec<&str> = r.skipped.iter().map(|(w, _)| w.as_str()).collect();
        assert_eq!(skipped, vec!["w1", "w2"]);
        assert_eq!(e.count("SELECT COUNT(*) FROM docs WHERE id = ?1", "p1"), 1);
        assert_eq!(e.count("SELECT COUNT(*) FROM docs WHERE id = ?1", "q1"), 1);
    }

    #[tokio::test]
    async fn loaded_documents_are_kept() {
        let e = env();
        e.workspace("w1", &[("live", false)]);
        e.page("w1", "open", &[], true);
        e.page("w1", "closed", &[], true);
        let reg = DocRegistry::new(e.db.clone(), e.dir.path().to_path_buf(), 0);
        let ws = reg.get_or_load("w1", "w1", false).unwrap();
        reg.get_or_load("open", "w1", false).unwrap();
        // The live workspace document is what counts: it knows `closed` now.
        workspace::create_page_with_id(&ws.doc, "closed", "C", "", "t").unwrap();
        let r = sweep(
            &e.db,
            e.dir.path(),
            Some(&reg),
            now_ms() + GRACE_MS + 1000,
            false,
        )
        .unwrap();
        assert!(r.docs_deleted.is_empty());
        assert_eq!(
            e.count("SELECT COUNT(*) FROM docs WHERE id = ?1", "open"),
            1
        );
        assert_eq!(
            e.count("SELECT COUNT(*) FROM docs WHERE id = ?1", "closed"),
            1
        );
    }
}
