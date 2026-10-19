//! Content-addressed image assets (`/api/workspaces/:id/assets/:name`),
//! stored at `<data>/assets/<workspace>/<sha256>.<ext>`. Page markdown refers
//! to them as `kawruh-asset://<workspace>/<name>`.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::api::{require_role, valid_id};
use crate::auth::AuthUser;
use crate::db::Role;
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// URL scheme of asset references in page markdown.
pub const SCHEME: &str = "kawruh-asset://";

pub const CACHE_PRIVATE: &str = "private, max-age=31536000, immutable";
pub const CACHE_PUBLIC: &str = "public, max-age=31536000, immutable";

/// Accepted extensions and the content type each is served with.
const TYPES: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
];

/// Parses `<sha256 hex lowercase>.<ext>` into (hash, content type).
pub fn parse_name(name: &str) -> Option<(&str, &'static str)> {
    let (hash, ext) = name.split_once('.')?;
    if hash.len() != 64 || !hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    TYPES
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, content_type)| (hash, *content_type))
}

pub fn workspace_dir(data_dir: &Path, workspace_id: &str) -> PathBuf {
    data_dir.join("assets").join(workspace_id)
}

/// Removes all assets of a workspace (nothing to do when it has none).
pub fn remove_workspace(data_dir: &Path, workspace_id: &str) -> std::io::Result<()> {
    if !valid_id(workspace_id) {
        return Ok(());
    }
    match std::fs::remove_dir_all(workspace_dir(data_dir, workspace_id)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Reads a stored asset; `None` when it does not exist.
pub async fn read(data_dir: &Path, workspace_id: &str, name: &str) -> ApiResult<Option<Vec<u8>>> {
    if !valid_id(workspace_id) || parse_name(name).is_none() {
        return Ok(None);
    }
    match tokio::fs::read(workspace_dir(data_dir, workspace_id).join(name)).await {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// An image response with long-lived caching (names are content hashes).
pub fn serve(bytes: Vec<u8>, content_type: &'static str, cache: &'static str) -> Response {
    let mut res = bytes.into_response();
    let headers = res.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

fn bad_name() -> ApiError {
    ApiError::bad_request(
        "bad_name",
        "asset name must be <sha256 hex>.<png|jpg|gif|webp|avif>",
    )
}

fn too_large(max: u64) -> ApiError {
    ApiError::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "too_large",
        format!("assets are limited to {max} bytes"),
    )
}

/// Streams `body` into `tmp`, enforcing `max`; returns (size, sha256 hex).
async fn receive(body: Body, tmp: &Path, max: u64) -> ApiResult<(u64, String)> {
    let mut file = tokio::fs::File::create(tmp)
        .await
        .map_err(ApiError::internal)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::bad_request("invalid", e.to_string()))?;
        size += chunk.len() as u64;
        if size > max {
            return Err(too_large(max));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    file.sync_all().await.map_err(ApiError::internal)?;
    Ok((size, hex::encode(hasher.finalize())))
}

/// Reads and discards up to `max` bytes of a body that will not be stored, so
/// the client gets the error response rather than a connection reset.
async fn drain(body: Body, max: u64) {
    let mut seen = 0u64;
    let mut stream = body.into_data_stream();
    while let Some(Ok(chunk)) = stream.next().await {
        seen += chunk.len() as u64;
        if seen > max {
            break;
        }
    }
}

/// Everything an upload must pass before its body is read.
fn admit(
    state: &AppState,
    me: Result<AuthUser, ApiError>,
    workspace_id: &str,
    name: &str,
) -> ApiResult<()> {
    let me = me?;
    require_role(state, workspace_id, &me.user, Role::Editor)?;
    parse_name(name).ok_or_else(bad_name)?;
    if !state.docs.writes_allowed() {
        return Err(ApiError::new(
            StatusCode::INSUFFICIENT_STORAGE,
            "disk_full",
            "server is low on disk space; asset not saved",
        ));
    }
    Ok(())
}

/// `PUT /api/workspaces/:id/assets/:name` — raw bytes; idempotent.
///
/// The body is read through the unlimited `Body` extractor (axum's default
/// 2 MB limit does not apply) and capped at `KAWRUH_MAX_ASSET_MB` here.
pub async fn upload(
    State(state): State<AppState>,
    me: Result<AuthUser, ApiError>,
    UrlPath((id, name)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Body,
) -> ApiResult<Response> {
    let max = state.cfg.max_asset_bytes();
    let declared = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let oversized = declared.is_some_and(|n| n > max);
    if let Err(e) = admit(&state, me, &id, &name) {
        if !oversized {
            drain(body, max).await;
        }
        return Err(e);
    }
    if oversized {
        return Err(too_large(max));
    }
    let (hash, _) = parse_name(&name).ok_or_else(bad_name)?;

    let dir = workspace_dir(&state.cfg.data_dir, &id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(ApiError::internal)?;
    let tmp = dir.join(format!(".{name}.{:016x}.tmp", rand::random::<u64>()));
    let received = receive(body, &tmp, max).await;
    let size = match received {
        Ok((size, actual)) if actual == hash => size,
        Ok(_) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(ApiError::bad_request(
                "bad_hash",
                "content does not match the sha256 in the asset name",
            ));
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };
    let dest = dir.join(&name);
    let existed = tokio::fs::try_exists(&dest).await.unwrap_or(false);
    if existed {
        let _ = tokio::fs::remove_file(&tmp).await;
    } else if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(ApiError::internal(e));
    }
    let status = if existed {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((status, Json(json!({ "name": name, "size": size }))).into_response())
}

/// `GET /api/workspaces/:id/assets/:name` — any member.
pub async fn download(
    State(state): State<AppState>,
    me: AuthUser,
    UrlPath((id, name)): UrlPath<(String, String)>,
) -> ApiResult<Response> {
    require_role(&state, &id, &me.user, Role::Viewer)?;
    let (_, content_type) = parse_name(&name).ok_or_else(bad_name)?;
    let bytes = read(&state.cfg.data_dir, &id, &name)
        .await?
        .ok_or_else(|| ApiError::not_found("not_found", "asset not found"))?;
    Ok(serve(bytes, content_type, CACHE_PRIVATE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "3f79bb7b435b05321651daefd374cdc681dc06faa65e374e38337b88ca046dea";

    #[test]
    fn names_are_hash_and_known_extension() {
        assert_eq!(
            parse_name(&format!("{HASH}.png")),
            Some((HASH, "image/png"))
        );
        assert_eq!(
            parse_name(&format!("{HASH}.jpg")).map(|p| p.1),
            Some("image/jpeg")
        );
        assert_eq!(
            parse_name(&format!("{HASH}.avif")).map(|p| p.1),
            Some("image/avif")
        );
        for bad in [
            format!("{HASH}.svg"),
            format!("{HASH}.PNG"),
            format!("{HASH}.png.png"),
            format!("{}.png", HASH.to_uppercase()),
            format!("{}.png", &HASH[1..]),
            HASH.to_string(),
            "../../etc/passwd.png".to_string(),
            "x.png".to_string(),
        ] {
            assert!(parse_name(&bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn receive_enforces_the_size_limit_without_content_length() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("t.tmp");
        let (size, hash) = receive(Body::from(vec![7u8; 100]), &tmp, 100)
            .await
            .unwrap();
        assert_eq!(size, 100);
        assert_eq!(hash, hex::encode(Sha256::digest([7u8; 100])));
        let err = receive(Body::from(vec![7u8; 101]), &tmp, 100)
            .await
            .unwrap_err();
        assert_eq!(
            (err.status, err.code),
            (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
        );
    }

    #[test]
    fn removing_a_workspace_without_assets_is_fine() {
        let dir = tempfile::tempdir().unwrap();
        remove_workspace(dir.path(), "nothing-here").unwrap();
        let ws = workspace_dir(dir.path(), "w1");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join(format!("{HASH}.png")), b"x").unwrap();
        remove_workspace(dir.path(), "w1").unwrap();
        assert!(!ws.exists());
        // Never escapes the assets directory.
        remove_workspace(dir.path(), "..").unwrap();
        assert!(dir.path().exists());
    }
}
