mod state;
mod errors;
mod db;
mod routes;

use std::sync::Arc;
use minijinja::Environment;
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

use state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present
    let _ = dotenvy::dotenv();

    // Init tracing
    fmt()
        .with_env_filter(EnvFilter::from_default_env()
            .add_directive("card_vault=info".parse()?)
            .add_directive("tower_http=info".parse()?))
        .init();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "sqlite://./card-vault.db".to_string());
    let scan_storage_path = std::env::var("SCAN_STORAGE_PATH")
        .unwrap_or_else(|_| "./scans".to_string());
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());

    // Create scan directory
    tokio::fs::create_dir_all(&scan_storage_path).await?;

    // DB pool
    let pool = db::create_pool(&database_url).await?;

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;
    info!("Migrations applied");

    // Set up minijinja
    let mut env = Environment::new();
    env.set_loader(minijinja::path_loader("templates"));
    let tmpl = Arc::new(env);

    let state = AppState {
        db: pool.clone(),
        tmpl,
        scan_storage_path: scan_storage_path.clone(),
    };

    // Spawn Scryfall hydration background task
    tokio::spawn(db::scryfall::hydrate_cards(pool));

    // Build router
    let app = routes::build_router(state)
        .nest_service("/scans", ServeDir::new(&scan_storage_path))
        .layer(TraceLayer::new_for_http());

    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("Listening on http://{addr}");

    axum::serve(listener, app).await?;
    Ok(())
}
