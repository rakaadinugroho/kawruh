//! Kawruh sync server: accounts, workspaces, real-time document sync,
//! public share pages and the MCP endpoint.

pub mod admin;
pub mod api;
pub mod auth;
pub mod config;
pub mod db;
pub mod disk;
pub mod docs;
pub mod error;
pub mod mcp;
pub mod share;
pub mod state;
pub mod ws;

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
        .route("/api/shares/{token}", delete(api::delete_share))
        .route("/api/ws", get(ws::handler))
        .route("/s/{token}", get(share::root))
        .route("/s/{token}/{page_id}", get(share::sub))
        .merge(mcp)
        .with_state(state)
}

/// Serves on an already-bound listener until the process ends.
pub async fn serve(listener: tokio::net::TcpListener, cfg: Config) -> anyhow::Result<()> {
    let state = AppState::new(cfg)?;
    axum::serve(listener, router(state)).await?;
    Ok(())
}
