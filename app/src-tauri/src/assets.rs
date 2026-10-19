//! Pasted images. Bytes live once in the local database, keyed by their
//! SHA-256; pages refer to them as `kawruh-asset://<workspace>/<sha256>.<ext>`.
//! The webview loads them through the `kawruh-asset` protocol, and assets of
//! server workspaces are uploaded there so collaborators can fetch them.

use std::collections::HashSet;
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use tauri::http::{Response, StatusCode, header};
use tauri::{AppHandle, Manager};

use crate::commands::AppState;
use crate::error::{Error, Result};

pub const SCHEME: &str = "kawruh-asset";
pub const PREFIX: &str = "kawruh-asset://";
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS assets (
            hash TEXT PRIMARY KEY,
            mime TEXT NOT NULL,
            bytes BLOB NOT NULL,
            created_at INTEGER NOT NULL
        );
        -- Assets a server workspace still has to upload.
        CREATE TABLE IF NOT EXISTS asset_uploads (
            workspace_id TEXT NOT NULL,
            name TEXT NOT NULL,
            PRIMARY KEY (workspace_id, name)
        );",
    )?;
    Ok(())
}

const TYPES: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
];

pub fn mime_for(ext: &str) -> Option<&'static str> {
    TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)
}

fn ext_for(mime: &str) -> Option<&'static str> {
    let mime = if mime == "image/jpg" {
        "image/jpeg"
    } else {
        mime
    };
    TYPES.iter().find(|(_, m)| *m == mime).map(|(e, _)| *e)
}

/// The type the bytes actually are, so a mislabelled file is stored and
/// served as what it is.
fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else if b.len() > 12 && &b[4..8] == b"ftyp" && (&b[8..12] == b"avif" || &b[8..12] == b"avis")
    {
        Some("avif")
    } else {
        None
    }
}

/// `<sha256>.<ext>` → (hash, ext), or None if it isn't an asset name.
pub fn parse_name(name: &str) -> Option<(&str, &str)> {
    let (hash, ext) = name.split_once('.')?;
    let ok = hash.len() == 64 && hash.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
    (ok && mime_for(ext).is_some()).then_some((hash, ext))
}

pub fn src(workspace_id: &str, name: &str) -> String {
    format!("{PREFIX}{workspace_id}/{name}")
}

/// `kawruh-asset://<ws>/<name>` → (ws, name).
pub fn parse_src(src: &str) -> Option<(&str, &str)> {
    let (ws, name) = src.strip_prefix(PREFIX)?.split_once('/')?;
    parse_name(name).map(|_| (ws, name))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Stores an image for `workspace_id` and returns its asset name.
pub fn put(conn: &Connection, workspace_id: &str, bytes: &[u8], mime: &str) -> Result<String> {
    if bytes.len() > MAX_BYTES {
        return Err(Error::msg("Images can be up to 10 MB."));
    }
    let ext = sniff(bytes)
        .or_else(|| ext_for(mime))
        .ok_or_else(|| Error::msg("Only PNG, JPEG, GIF, WebP and AVIF images can be pasted."))?;
    let name = format!("{}.{ext}", sha256_hex(bytes));
    store(conn, &name, bytes)?;
    conn.execute(
        "INSERT OR IGNORE INTO asset_uploads (workspace_id, name) VALUES (?1, ?2)",
        params![workspace_id, name],
    )?;
    Ok(name)
}

fn store(conn: &Connection, name: &str, bytes: &[u8]) -> Result<()> {
    let (hash, ext) = parse_name(name).ok_or_else(|| Error::msg("bad asset name"))?;
    conn.execute(
        "INSERT OR IGNORE INTO assets (hash, mime, bytes, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            hash,
            mime_for(ext).unwrap_or("application/octet-stream"),
            bytes,
            kawruh_core::workspace::now_ms()
        ],
    )?;
    Ok(())
}

pub fn get(conn: &Connection, hash: &str) -> Result<Option<(String, Vec<u8>)>> {
    Ok(conn
        .query_row(
            "SELECT mime, bytes FROM assets WHERE hash = ?1",
            [hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn pending(conn: &Connection, workspace_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM asset_uploads WHERE workspace_id = ?1")?;
    let rows = stmt.query_map([workspace_id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn mark_uploaded(conn: &Connection, workspace_id: &str, name: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM asset_uploads WHERE workspace_id = ?1 AND name = ?2",
        params![workspace_id, name],
    )?;
    Ok(())
}

/// The asset's bytes, fetched from the workspace's server (and cached) when
/// they aren't here yet.
pub async fn load(
    app: &AppHandle,
    workspace_id: &str,
    name: &str,
) -> Result<Option<(String, Vec<u8>)>> {
    let (hash, _) = parse_name(name).ok_or_else(|| Error::msg("bad asset name"))?;
    let state = app.state::<AppState>();
    if let Some(found) = get(&state.hub.db.lock().unwrap(), hash)? {
        return Ok(Some(found));
    }
    let Ok(account) = state.workspace_account(workspace_id) else {
        return Ok(None);
    };
    let Some(bytes) = state
        .remote
        .get_asset(&account.server_url, &account.token, workspace_id, name)
        .await?
    else {
        return Ok(None);
    };
    if bytes.len() > MAX_BYTES || sha256_hex(&bytes) != hash {
        return Err(Error::msg(format!("server sent a corrupt copy of {name}")));
    }
    let db = state.hub.db.lock().unwrap();
    store(&db, name, &bytes)?;
    get(&db, hash)
}

/// Serves `kawruh-asset://localhost/<ws>%2F<name>` (Windows:
/// `http://kawruh-asset.localhost/...`) to the webview.
pub async fn serve(app: &AppHandle, path: &str) -> Response<Vec<u8>> {
    let decoded = percent_encoding::percent_decode_str(path.trim_start_matches('/'))
        .decode_utf8_lossy()
        .to_string();
    let reply = |status: StatusCode, mime: &str, body: Vec<u8>| {
        Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "max-age=31536000, immutable")
            .header("X-Content-Type-Options", "nosniff")
            .body(body)
            .unwrap()
    };
    let Some((ws, name)) = decoded
        .split_once('/')
        .filter(|(_, n)| parse_name(n).is_some())
    else {
        return reply(
            StatusCode::BAD_REQUEST,
            "text/plain",
            b"bad asset path".to_vec(),
        );
    };
    match load(app, ws, name).await {
        Ok(Some((mime, bytes))) => reply(StatusCode::OK, &mime, bytes),
        Ok(None) => reply(StatusCode::NOT_FOUND, "text/plain", b"not found".to_vec()),
        Err(e) => {
            tracing::warn!("asset {decoded}: {e}");
            reply(
                StatusCode::BAD_GATEWAY,
                "text/plain",
                e.to_string().into_bytes(),
            )
        }
    }
}

static FLUSHING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Uploads the workspace's pending assets to its server. A failure leaves the
/// rest queued for the next time the workspace comes online.
pub async fn flush(app: AppHandle, workspace_id: String) {
    {
        let mut running = FLUSHING.lock().unwrap();
        if !running
            .get_or_insert_with(HashSet::new)
            .insert(workspace_id.clone())
        {
            return;
        }
    }
    if let Err(e) = upload_pending(&app, &workspace_id).await {
        tracing::warn!("uploading images of {workspace_id}: {e}");
    }
    if let Some(running) = FLUSHING.lock().unwrap().as_mut() {
        running.remove(&workspace_id);
    }
}

async fn upload_pending(app: &AppHandle, workspace_id: &str) -> Result<()> {
    let state = app.state::<AppState>();
    let Ok(account) = state.workspace_account(workspace_id) else {
        return Ok(()); // local workspace: uploads wait for publish
    };
    let names = pending(&state.hub.db.lock().unwrap(), workspace_id)?;
    for name in names {
        let Some((hash, ext)) = parse_name(&name) else {
            continue;
        };
        let found = get(&state.hub.db.lock().unwrap(), hash)?;
        if let Some((_, bytes)) = found {
            state
                .remote
                .put_asset(
                    &account.server_url,
                    &account.token,
                    workspace_id,
                    &name,
                    bytes,
                    mime_for(ext).unwrap_or("application/octet-stream"),
                )
                .await?;
        }
        mark_uploaded(&state.hub.db.lock().unwrap(), workspace_id, &name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        conn
    }

    #[test]
    fn stores_by_content_and_queues_one_upload() {
        let conn = db();
        // Declared as JPEG, but the bytes are a PNG.
        let a = put(&conn, "w1", PNG, "image/jpeg").unwrap();
        let b = put(&conn, "w1", PNG, "image/png").unwrap();
        assert_eq!(a, b);
        assert!(a.ends_with(".png"));
        assert_eq!(pending(&conn, "w1").unwrap(), vec![a.clone()]);
        let (hash, _) = parse_name(&a).unwrap();
        assert_eq!(get(&conn, hash).unwrap().unwrap().0, "image/png");
        mark_uploaded(&conn, "w1", &a).unwrap();
        assert!(pending(&conn, "w1").unwrap().is_empty());
    }

    #[test]
    fn rejects_non_images_and_oversized_files() {
        let conn = db();
        assert!(put(&conn, "w1", b"hello", "text/plain").is_err());
        let big = [PNG, &vec![0u8; MAX_BYTES]].concat();
        assert!(put(&conn, "w1", &big, "image/png").is_err());
    }

    #[test]
    fn names_and_srcs_are_strict() {
        let name = format!("{}.png", "a".repeat(64));
        assert!(parse_name(&name).is_some());
        assert!(parse_name("../etc/passwd.png").is_none());
        assert!(parse_name(&format!("{}.svg", "a".repeat(64))).is_none());
        assert_eq!(parse_src(&src("ws", &name)), Some(("ws", name.as_str())));
        assert!(parse_src("https://example.com/a.png").is_none());
    }
}
