//! REST endpoints (docs/API.md).

use axum::Json;
use axum::extract::{FromRequest, Path, Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use rand::RngCore;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};

use kawruh_core::workspace::{self, now_ms};

use crate::auth::{self, AuthUser, PAT_PREFIX};
use crate::db::{self, Role, User};
use crate::docs::ORIGIN_SERVER;
use crate::error::{ApiError, ApiResult};
use crate::ratelimit::ClientIp;
use crate::state::AppState;

/// `Json` with rejections mapped to the API error format.
pub struct Body<T>(pub T);

impl<S, T> FromRequest<S> for Body<T>
where
    T: serde::de::DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(v)) => Ok(Body(v)),
            Err(e) => Err(ApiError::bad_request("invalid", e.body_text())),
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_username(u: &str) -> bool {
    (3..=32).contains(&u.len())
        && u.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
}

fn random_hex(n: usize) -> String {
    let mut bytes = vec![0u8; n];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// The caller's role in a workspace, or 404 / 403.
pub fn require_role(
    state: &AppState,
    workspace_id: &str,
    user: &User,
    min: Role,
) -> ApiResult<Role> {
    let conn = state.db.lock();
    if db::workspace_owner(&conn, workspace_id)?.is_none() {
        return Err(ApiError::not_found(
            "workspace_not_found",
            "workspace not found",
        ));
    }
    let role = db::role_of(&conn, workspace_id, &user.id)?
        .ok_or_else(|| ApiError::forbidden("not a member of this workspace"))?;
    if role < min {
        return Err(ApiError::forbidden(format!(
            "requires {} role",
            min.as_str()
        )));
    }
    Ok(role)
}

fn issue_session(state: &AppState, user: &User) -> ApiResult<String> {
    let token = auth::new_token("");
    let conn = state.db.lock();
    db::insert_token(
        &conn,
        &workspace::new_id(),
        &user.id,
        "session",
        "session",
        &auth::hash_token(&token),
    )?;
    Ok(token)
}

// ---- auth -------------------------------------------------------------------

#[derive(Deserialize)]
pub struct RegisterReq {
    username: String,
    password: String,
    display_name: Option<String>,
    invite: Option<String>,
}

/// With sign-up closed, every account but the very first needs an invite
/// code. A valid code is used up, so call this inside the transaction that
/// creates the user; with `invite: None` it only checks.
fn admit_signup(conn: &Connection, allow_signup: bool, invite: Option<&str>) -> ApiResult<()> {
    if allow_signup || db::user_count(conn)? == 0 {
        return Ok(());
    }
    let code = invite.ok_or_else(|| {
        ApiError::forbidden("this server needs an invite code to sign up")
            .with_code("invite_required")
    })?;
    if !db::consume_invite(conn, code, now_ms())? {
        return Err(
            ApiError::forbidden("the invite code is invalid, expired or already used")
                .with_code("invite_invalid"),
        );
    }
    Ok(())
}

pub async fn register(
    State(state): State<AppState>,
    Body(req): Body<RegisterReq>,
) -> ApiResult<Json<Value>> {
    let invite = req
        .invite
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string);
    if invite.is_none() {
        admit_signup(&state.db.lock(), state.cfg.allow_signup, None)?;
    }
    let username = req.username.trim().to_lowercase();
    if !valid_username(&username) {
        return Err(ApiError::bad_request(
            "invalid",
            "username must be 3-32 chars of a-z 0-9 _ . -",
        ));
    }
    if req.password.chars().count() < 8 {
        return Err(ApiError::bad_request(
            "invalid",
            "password must be at least 8 characters",
        ));
    }
    let display_name = req
        .display_name
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| username.clone());
    let password = req.password.clone();
    let hash = tokio::task::spawn_blocking(move || auth::hash_password(&password))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::internal)?;
    let user = User {
        id: workspace::new_id(),
        username,
        display_name,
    };
    {
        let mut conn = state.db.lock();
        let tx = conn.transaction()?;
        admit_signup(&tx, state.cfg.allow_signup, invite.as_deref())?;
        if db::user_by_username(&tx, &user.username)?.is_some() {
            return Err(ApiError::conflict(
                "username_taken",
                "username is already taken",
            ));
        }
        db::insert_user(&tx, &user, &hash)?;
        tx.commit()?;
    }
    let token = issue_session(&state, &user)?;
    Ok(Json(json!({ "token": token, "user": user })))
}

#[derive(Deserialize)]
pub struct LoginReq {
    username: String,
    password: String,
}

pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Body(req): Body<LoginReq>,
) -> ApiResult<Json<Value>> {
    let username = req.username.trim().to_lowercase();
    if let Some(wait) = state.logins.blocked(&username, ip) {
        return Err(ApiError::too_many_attempts(wait));
    }
    let found = {
        let conn = state.db.lock();
        db::user_by_username(&conn, &username)?
    };
    let invalid = || {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "wrong username or password",
        )
    };
    let Some((user, hash)) = found else {
        state.logins.record_failure(&username, ip);
        return Err(invalid());
    };
    let password = req.password.clone();
    let ok = tokio::task::spawn_blocking(move || auth::verify_password(&password, &hash))
        .await
        .map_err(ApiError::internal)?;
    if !ok {
        state.logins.record_failure(&username, ip);
        return Err(invalid());
    }
    state.logins.clear(&username);
    let token = issue_session(&state, &user)?;
    Ok(Json(json!({ "token": token, "user": user })))
}

pub async fn logout(State(state): State<AppState>, me: AuthUser) -> ApiResult<Response> {
    if me.token_kind == "session" {
        let conn = state.db.lock();
        conn.execute("DELETE FROM tokens WHERE id = ?1", [&me.token_id])?;
    }
    Ok(no_content())
}

/// The user plus their asset storage: bytes used across the workspaces they
/// own, and the quota (0 = unlimited).
pub async fn me(State(state): State<AppState>, me: AuthUser) -> ApiResult<Json<Value>> {
    let used = crate::assets::owner_usage(&state, &me.user.id)?;
    Ok(Json(json!({
        "id": me.user.id,
        "username": me.user.username,
        "display_name": me.user.display_name,
        "storage_used_bytes": used,
        "storage_quota_bytes": state.cfg.quota_bytes(),
    })))
}

// ---- invites ----------------------------------------------------------------

pub async fn create_invite(State(state): State<AppState>, me: AuthUser) -> ApiResult<Json<Value>> {
    let conn = state.db.lock();
    let invite = db::create_invite(&conn, Some(&me.user.id), now_ms())?;
    Ok(Json(
        json!({ "code": invite.code, "expires_at": invite.expires_at }),
    ))
}

pub async fn list_invites(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<Json<Vec<db::Invite>>> {
    let conn = state.db.lock();
    Ok(Json(db::invites_of(&conn, &me.user.id, now_ms())?))
}

pub async fn delete_invite(
    State(state): State<AppState>,
    me: AuthUser,
    Path(code): Path<String>,
) -> ApiResult<Response> {
    let conn = state.db.lock();
    if !db::delete_invite(&conn, &me.user.id, &code)? {
        return Err(ApiError::not_found("invite_not_found", "invite not found"));
    }
    Ok(no_content())
}

// ---- personal access tokens -------------------------------------------------

pub async fn list_tokens(State(state): State<AppState>, me: AuthUser) -> ApiResult<Json<Value>> {
    let conn = state.db.lock();
    let mut stmt = conn.prepare(
        "SELECT id, name, created_at, last_used_at FROM tokens WHERE user_id = ?1 AND kind = 'pat' ORDER BY created_at",
    )?;
    let rows = stmt
        .query_map([&me.user.id], |r| {
            Ok(json!({
                "id": r.get::<_, String>(0)?,
                "name": r.get::<_, String>(1)?,
                "created_at": r.get::<_, i64>(2)?,
                "last_used_at": r.get::<_, Option<i64>>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Json(Value::Array(rows)))
}

#[derive(Deserialize)]
pub struct TokenReq {
    name: String,
}

pub async fn create_token(
    State(state): State<AppState>,
    me: AuthUser,
    Body(req): Body<TokenReq>,
) -> ApiResult<Json<Value>> {
    let name = req.name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err(ApiError::bad_request(
            "invalid",
            "token name must be 1-64 characters",
        ));
    }
    let token = auth::new_token(PAT_PREFIX);
    let id = workspace::new_id();
    let conn = state.db.lock();
    db::insert_token(
        &conn,
        &id,
        &me.user.id,
        "pat",
        name,
        &auth::hash_token(&token),
    )?;
    Ok(Json(json!({ "id": id, "name": name, "token": token })))
}

pub async fn delete_token(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let conn = state.db.lock();
    let n = conn.execute(
        "DELETE FROM tokens WHERE id = ?1 AND user_id = ?2 AND kind = 'pat'",
        params![id, me.user.id],
    )?;
    if n == 0 {
        return Err(ApiError::not_found("token_not_found", "token not found"));
    }
    Ok(no_content())
}

// ---- workspaces ---------------------------------------------------------------

pub async fn list_workspaces(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<Json<Vec<db::WorkspaceRow>>> {
    let conn = state.db.lock();
    Ok(Json(db::workspaces_of(&conn, &me.user.id)?))
}

#[derive(Deserialize)]
pub struct WorkspaceReq {
    id: Option<String>,
    name: String,
}

pub async fn create_workspace(
    State(state): State<AppState>,
    me: AuthUser,
    Body(req): Body<WorkspaceReq>,
) -> ApiResult<Json<db::WorkspaceRow>> {
    let name = req.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(ApiError::bad_request(
            "invalid",
            "workspace name must be 1-100 characters",
        ));
    }
    let publishing = req.id.is_some();
    let id = match req.id {
        Some(id) if valid_id(&id) => id,
        Some(_) => return Err(ApiError::bad_request("invalid", "invalid workspace id")),
        None => workspace::new_id(),
    };
    {
        let conn = state.db.lock();
        if let Some(owner) = db::workspace_owner(&conn, &id)? {
            if owner != me.user.id {
                return Err(ApiError::conflict(
                    "workspace_exists",
                    "workspace id already in use",
                ));
            }
            let name: String =
                conn.query_row("SELECT name FROM workspaces WHERE id = ?1", [&id], |r| {
                    r.get(0)
                })?;
            return Ok(Json(db::WorkspaceRow {
                id,
                name,
                role: Role::Owner,
            }));
        }
        if kawruh_core::store::doc_workspace(&conn, &id)?.is_some() {
            return Err(ApiError::conflict(
                "workspace_exists",
                "id already used by a document",
            ));
        }
        conn.execute(
            "INSERT INTO workspaces (id, name, owner_id, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![id, name, me.user.id, now_ms()],
        )?;
        conn.execute(
            "INSERT INTO members (workspace_id, user_id, role) VALUES (?1, ?2, 'owner')",
            params![id, me.user.id],
        )?;
    }
    if !publishing {
        // A fresh remote workspace: seed the workspace doc's name.
        let live = state
            .docs
            .get_or_load(&id, &id, true)
            .map_err(ApiError::internal)?;
        workspace::set_workspace_name(&live.doc, &name, ORIGIN_SERVER);
    }
    Ok(Json(db::WorkspaceRow {
        id,
        name,
        role: Role::Owner,
    }))
}

pub async fn delete_workspace(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    require_role(&state, &id, &me.user, Role::Owner)?;
    state.docs.close_workspace(&id);
    {
        let mut conn = state.db.lock();
        conn.execute("DELETE FROM page_text WHERE workspace_id = ?1", [&id])?;
        conn.execute("DELETE FROM workspaces WHERE id = ?1", [&id])?;
        kawruh_core::store::delete_workspace_docs(&mut conn, &id)?;
    }
    if let Err(e) = crate::assets::remove_workspace(&state.cfg.data_dir, &id) {
        tracing::warn!("removing assets of workspace {id} failed: {e}");
    }
    Ok(no_content())
}

// ---- members --------------------------------------------------------------------

pub async fn list_members(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Vec<db::Member>>> {
    require_role(&state, &id, &me.user, Role::Viewer)?;
    let conn = state.db.lock();
    Ok(Json(db::members_of(&conn, &id)?))
}

#[derive(Deserialize)]
pub struct MemberReq {
    username: String,
    role: String,
}

pub async fn add_member(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
    Body(req): Body<MemberReq>,
) -> ApiResult<Json<db::Member>> {
    require_role(&state, &id, &me.user, Role::Owner)?;
    let role = match Role::parse(&req.role) {
        Some(r @ (Role::Editor | Role::Viewer)) => r,
        _ => {
            return Err(ApiError::bad_request(
                "invalid",
                "role must be editor or viewer",
            ));
        }
    };
    let conn = state.db.lock();
    let (user, _) = db::user_by_username(&conn, &req.username.trim().to_lowercase())?
        .ok_or_else(|| ApiError::not_found("user_not_found", "no user with that username"))?;
    if db::workspace_owner(&conn, &id)?.as_deref() == Some(user.id.as_str()) {
        return Err(ApiError::bad_request(
            "cannot_change_owner",
            "the owner's role cannot be changed",
        ));
    }
    conn.execute(
        "INSERT INTO members (workspace_id, user_id, role) VALUES (?1, ?2, ?3)
         ON CONFLICT(workspace_id, user_id) DO UPDATE SET role = ?3",
        params![id, user.id, role.as_str()],
    )?;
    Ok(Json(db::Member {
        user_id: user.id,
        username: user.username,
        display_name: user.display_name,
        role,
    }))
}

pub async fn remove_member(
    State(state): State<AppState>,
    me: AuthUser,
    Path((id, user_id)): Path<(String, String)>,
) -> ApiResult<Response> {
    require_role(&state, &id, &me.user, Role::Owner)?;
    let conn = state.db.lock();
    if db::workspace_owner(&conn, &id)?.as_deref() == Some(user_id.as_str()) {
        return Err(ApiError::bad_request(
            "cannot_remove_owner",
            "the owner cannot be removed",
        ));
    }
    conn.execute(
        "DELETE FROM members WHERE workspace_id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    Ok(no_content())
}

// ---- shares ----------------------------------------------------------------------

fn share_json(state: &AppState, s: &db::Share) -> Value {
    json!({
        "token": s.token,
        "page_id": s.page_id,
        "url": state.share_url(&s.token),
        "created_at": s.created_at,
    })
}

pub async fn list_shares(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    require_role(&state, &id, &me.user, Role::Viewer)?;
    let conn = state.db.lock();
    let shares = db::shares_of(&conn, &id)?;
    Ok(Json(Value::Array(
        shares.iter().map(|s| share_json(&state, s)).collect(),
    )))
}

#[derive(Deserialize)]
pub struct ShareReq {
    page_id: String,
}

/// The share link of a live page, created when it has none. The caller
/// checks the editor role.
pub fn ensure_share(
    state: &AppState,
    workspace_id: &str,
    page_id: &str,
    user: &User,
) -> ApiResult<db::Share> {
    let ws = state
        .docs
        .get_or_load(workspace_id, workspace_id, false)
        .map_err(ApiError::internal)?;
    if !workspace::live_pages(&ws.doc)
        .iter()
        .any(|p| p.id == page_id)
    {
        return Err(ApiError::not_found(
            "page_not_found",
            "page not found in this workspace",
        ));
    }
    let conn = state.db.lock();
    let existing = db::shares_of(&conn, workspace_id)?
        .into_iter()
        .find(|s| s.page_id == page_id);
    if let Some(s) = existing {
        return Ok(s);
    }
    let s = db::Share {
        token: random_hex(16),
        workspace_id: workspace_id.to_string(),
        page_id: page_id.to_string(),
        created_at: now_ms(),
    };
    conn.execute(
        "INSERT INTO shares (token, workspace_id, page_id, created_by, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![s.token, s.workspace_id, s.page_id, user.id, s.created_at],
    )?;
    Ok(s)
}

pub async fn create_share(
    State(state): State<AppState>,
    me: AuthUser,
    Path(id): Path<String>,
    Body(req): Body<ShareReq>,
) -> ApiResult<Json<Value>> {
    require_role(&state, &id, &me.user, Role::Editor)?;
    let share = ensure_share(&state, &id, &req.page_id, &me.user)?;
    let mut v = share_json(&state, &share);
    if let Some(o) = v.as_object_mut() {
        o.remove("created_at");
    }
    Ok(Json(v))
}

pub async fn delete_share(
    State(state): State<AppState>,
    me: AuthUser,
    Path(token): Path<String>,
) -> ApiResult<Response> {
    let share = {
        let conn = state.db.lock();
        db::share_by_token(&conn, &token)?
    }
    .ok_or_else(|| ApiError::not_found("share_not_found", "share link not found"))?;
    require_role(&state, &share.workspace_id, &me.user, Role::Editor)?;
    let conn = state.db.lock();
    conn.execute("DELETE FROM shares WHERE token = ?1", [&token])?;
    Ok(no_content())
}

// ---- health ------------------------------------------------------------------------

pub async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "disk_free_bytes": crate::disk::free_bytes(&state.cfg.data_dir),
    }))
}

impl ApiError {
    pub fn with_code(mut self, code: &'static str) -> ApiError {
        self.code = code;
        self
    }
}
