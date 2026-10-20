//! Server configuration from the environment (see docs/API.md).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub data_dir: PathBuf,
    pub public_url: String,
    pub allow_signup: bool,
    pub min_free_mb: u64,
    pub max_asset_mb: u64,
    /// Asset storage per workspace owner, in MB; 0 = unlimited.
    pub quota_mb: u64,
    /// Failed logins allowed per username or client IP within `login_window`
    /// (10 per 15 minutes; not read from the environment, tests shrink them).
    pub login_max_failures: u32,
    pub login_window: Duration,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Config> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Ok(Config {
            bind: var("KAWRUH_BIND")
                .unwrap_or_else(|| "0.0.0.0:7420".into())
                .parse()?,
            data_dir: var("KAWRUH_DATA").unwrap_or_else(|| "./data".into()).into(),
            public_url: var("KAWRUH_PUBLIC_URL")
                .unwrap_or_else(|| "http://localhost:7420".into())
                .trim_end_matches('/')
                .to_string(),
            allow_signup: var("KAWRUH_ALLOW_SIGNUP")
                .map(|v| {
                    !matches!(
                        v.to_ascii_lowercase().as_str(),
                        "0" | "false" | "no" | "off"
                    )
                })
                .unwrap_or(true),
            min_free_mb: var("KAWRUH_MIN_FREE_MB")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(50),
            max_asset_mb: var("KAWRUH_MAX_ASSET_MB")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(10),
            quota_mb: var("KAWRUH_QUOTA_MB")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(0),
            login_max_failures: 10,
            login_window: Duration::from_secs(15 * 60),
        })
    }

    /// Largest accepted asset upload, in bytes.
    pub fn max_asset_bytes(&self) -> u64 {
        self.max_asset_mb.saturating_mul(1024 * 1024)
    }

    /// Asset storage allowed per workspace owner, in bytes; 0 = unlimited.
    pub fn quota_bytes(&self) -> u64 {
        self.quota_mb.saturating_mul(1024 * 1024)
    }
}
