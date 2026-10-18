use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cfg = kawruh_server::Config::from_env()?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("admin") {
        let db = kawruh_server::db::Db::open(&cfg.data_dir.join("kawruh.db"))?;
        return kawruh_server::admin::run(&args[1..], &mut db.lock());
    }

    let listener = tokio::net::TcpListener::bind(cfg.bind).await?;
    tracing::info!(
        "kawruh-server {} listening on {} (data: {}, public: {})",
        env!("CARGO_PKG_VERSION"),
        cfg.bind,
        cfg.data_dir.display(),
        cfg.public_url
    );
    kawruh_server::serve(listener, cfg).await
}
