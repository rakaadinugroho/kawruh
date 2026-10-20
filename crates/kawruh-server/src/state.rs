//! Shared application state.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::db::Db;
use crate::docs::{ConnId, DocRegistry};
use crate::ratelimit::LoginLimiter;

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub docs: Arc<DocRegistry>,
    pub logins: Arc<LoginLimiter>,
    conn_ids: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(cfg: Config) -> anyhow::Result<AppState> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let db = Db::open(&cfg.data_dir.join("kawruh.db"))?;
        let docs = DocRegistry::new(db.clone(), cfg.data_dir.clone(), cfg.min_free_mb);
        let logins = Arc::new(LoginLimiter::new(cfg.login_max_failures, cfg.login_window));
        Ok(AppState {
            cfg: Arc::new(cfg),
            db,
            docs,
            logins,
            conn_ids: Arc::new(AtomicU64::new(1)),
        })
    }

    pub fn next_conn_id(&self) -> ConnId {
        self.conn_ids.fetch_add(1, Ordering::Relaxed)
    }

    pub fn share_url(&self, token: &str) -> String {
        format!("{}/s/{token}", self.cfg.public_url)
    }
}
