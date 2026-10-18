//! Shared application state.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::Config;
use crate::db::Db;
use crate::docs::{ConnId, DocRegistry};

#[derive(Clone)]
pub struct AppState {
    pub cfg: Arc<Config>,
    pub db: Db,
    pub docs: Arc<DocRegistry>,
    conn_ids: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(cfg: Config) -> anyhow::Result<AppState> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let db = Db::open(&cfg.data_dir.join("kawruh.db"))?;
        let docs = DocRegistry::new(db.clone(), cfg.data_dir.clone(), cfg.min_free_mb);
        Ok(AppState {
            cfg: Arc::new(cfg),
            db,
            docs,
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
