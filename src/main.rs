use shopcore::{config::Config, db, state::AppState};
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "shopcore=info".into()),
        )
        .init();
    if let Err(error) = run().await {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env();
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(15))
        .connect(&config.database_url)
        .await?;
    let bind_addr = config.bind_addr.clone();
    let state = AppState::new(pool, config);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() && args[0] != "serve" {
        shopcore::cli::run(&args, &state).await?;
        return Ok(());
    }
    db::init(&state.pool).await?;
    db::seed(&state.pool, &state.config).await?;
    tokio::fs::create_dir_all(&state.config.export_dir).await?;
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!(%bind_addr, "shopcore listening");
    axum::serve(listener, shopcore::app(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
