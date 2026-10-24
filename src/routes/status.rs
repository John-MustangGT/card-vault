use axum::{extract::State, Json};
use std::sync::Arc;

use crate::AppState;

pub async fn status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let (total_cards, unique_cards): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(quantity), 0), COUNT(DISTINCT scryfall_id)
         FROM inventory_lots WHERE quantity > 0",
    )
    .fetch_one(&state.pool)
    .await
    .unwrap_or((0, 0));

    let refresh: Option<i64> = sqlx::query_scalar("SELECT MAX(scraped_at) FROM price_history")
        .fetch_one(&state.pool)
        .await
        .ok()
        .flatten();
    let bulk: Option<i64> = sqlx::query_scalar("SELECT MAX(imported_at) FROM scryfall_bulk_imports")
        .fetch_one(&state.pool)
        .await
        .ok()
        .flatten();
    let (price_at, price_source) = match (refresh, bulk) {
        (Some(r), Some(b)) if r >= b => (Some(r), "Scryfall refresh"),
        (_, Some(b)) => (Some(b), "bulk import"),
        (Some(r), None) => (Some(r), "Scryfall refresh"),
        (None, None) => (None, ""),
    };

    Json(serde_json::json!({
        "total_cards": total_cards,
        "unique_cards": unique_cards,
        "last_price_update": price_at,
        "last_price_source": price_source,
        "build": {
            "version": env!("CARGO_PKG_VERSION"),
            "sha": env!("BUILD_GIT_SHA"),
            "dirty": env!("BUILD_GIT_DIRTY") == "true",
            "branch": env!("BUILD_GIT_BRANCH"),
            "built_at": env!("BUILD_TIMESTAMP").parse::<i64>().unwrap_or(0),
        },
    }))
}
