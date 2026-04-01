mod config;
mod db;
mod models;
mod routes;

use anyhow::Result;
use axum::{
    routing::{get, post},
    Router,
};
use minijinja::Environment;
use sqlx::SqlitePool;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub config: config::Config,
    pub env: Arc<Environment<'static>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "card_vault=debug,tower_http=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = config::Config::from_env()?;
    info!("Starting card-vault on {}:{}", config.host, config.port);

    // Ensure scan storage directory exists
    std::fs::create_dir_all(&config.scan_storage_path)?;

    let pool = db::init_pool(&config.database_url).await?;
    info!("Database initialized");

    let mut env = Environment::new();
    env.set_loader(minijinja::path_loader("templates"));
    env.add_filter("usd", |v: f64| -> String { format!("{:.2}", v) });
    env.add_filter("f1",  |v: f64| -> String { format!("{:.1}", v) });
    let env = Arc::new(env);

    let state = Arc::new(AppState {
        pool,
        config: config.clone(),
        env,
    });

    // ── Background bulk import task (runs immediately, then every 30 min) ───
    {
        let bg_pool = state.pool.clone();
        let bg_data_dir = state.config.data_dir.clone();
        tokio::spawn(async move {
            loop {
                match db::bulk::run_import(&bg_pool, &bg_data_dir).await {
                    Ok(n) if n > 0 => info!("bulk import background: {} new files processed", n),
                    Ok(_) => info!("bulk import background: no new files"),
                    Err(e) => tracing::warn!("bulk import background error: {}", e),
                }
                tokio::time::sleep(Duration::from_secs(30 * 60)).await;
            }
        });
    }

    let app = Router::new()
        .route("/", get(|| async { axum::response::Redirect::to("/import") }))
        .route("/import", get(routes::import::import_page))
        .route("/import", post(routes::import::handle_import))
        .route("/inventory", get(routes::inventory::inventory_page))
        .route("/inventory/refresh-prices", post(routes::inventory::refresh_prices))
        .route("/individuals", get(routes::individuals::individuals_page))
        .route("/inventory/card/:scryfall_id", get(routes::inventory::card_detail))
        .route("/inventory/card/:scryfall_id/individual", post(routes::inventory::create_individual))
        .route("/individuals/:id/status", post(routes::inventory::update_individual_status))
        .route("/locations", get(routes::locations::locations_page))
        .route("/locations", post(routes::locations::create_location))
        .route("/locations/:id/delete", post(routes::locations::delete_location))
        .route("/market", get(routes::market::market_page))
        .route("/market/import", post(routes::market::trigger_import))
        .route("/market/search", get(routes::market::search_prices))
        .route("/market/clear", post(routes::market::clear_market))
        .route("/sales", get(routes::sales::sales_page))
        .route("/sales", post(routes::sales::create_sale))
        .route("/sales/new", get(routes::sales::new_sale_page))
        .route("/sales/:id", get(routes::sales::sale_detail))
        .route("/sales/:id/label", get(routes::sales::sale_label))
        // Static files
        .nest_service("/static", ServeDir::new("static"))
        // Scan image serving
        .nest_service("/scans", ServeDir::new(&config.scan_storage_path))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    info!("Listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
