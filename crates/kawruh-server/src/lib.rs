//! Kawruh sync server: accounts, workspaces, real-time document sync,
//! public share pages and the MCP endpoint.

pub mod admin;
pub mod api;
pub mod assets;
pub mod auth;
pub mod config;
pub mod db;
pub mod disk;
pub mod docs;
pub mod error;
pub mod gc;
pub mod mcp;
pub mod notify;
pub mod ratelimit;
pub mod share;
pub mod state;
pub mod ws;

use std::net::SocketAddr;

use axum::Router;
use axum::routing::{delete, get, post};

pub use config::Config;
pub use state::AppState;

pub fn router(state: AppState) -> Router {
    let mcp = Router::new()
        .nest_service("/mcp", mcp::service(state.clone()))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            mcp::require_auth,
        ));

    Router::new()
        .route("/api/health", get(api::health))
        .route("/api/auth/register", post(api::register))
        .route("/api/auth/login", post(api::login))
        .route("/api/auth/logout", post(api::logout))
        .route("/api/me", get(api::me))
        .route("/api/tokens", get(api::list_tokens).post(api::create_token))
        .route("/api/tokens/{id}", delete(api::delete_token))
        .route(
            "/api/invites",
            get(api::list_invites).post(api::create_invite),
        )
        .route("/api/invites/{code}", delete(api::delete_invite))
        .route(
            "/api/workspaces",
            get(api::list_workspaces).post(api::create_workspace),
        )
        .route("/api/workspaces/{id}", delete(api::delete_workspace))
        .route(
            "/api/workspaces/{id}/members",
            get(api::list_members).post(api::add_member),
        )
        .route(
            "/api/workspaces/{id}/members/{user_id}",
            delete(api::remove_member),
        )
        .route(
            "/api/workspaces/{id}/shares",
            get(api::list_shares).post(api::create_share),
        )
        .route(
            "/api/workspaces/{id}/assets/{name}",
            get(assets::download).put(assets::upload),
        )
        .route("/api/notifications", get(api::list_notifications))
        .route("/api/notifications/read", post(api::read_notifications))
        .route("/api/shares/{token}", delete(api::delete_share))
        .route("/api/ws", get(ws::handler))
        .route("/s/{token}", get(share::root))
        .route("/s/{token}/{page_id}", get(share::sub))
        .route("/s/{token}/assets/{name}", get(share::asset))
        .merge(mcp)
        .with_state(state)
}

/// Serves on an already-bound listener until the process ends. Handlers see
/// the peer address (`ConnectInfo`), used for login rate limiting.
pub async fn serve(listener: tokio::net::TcpListener, cfg: Config) -> anyhow::Result<()> {
    let state = AppState::new(cfg)?;
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
