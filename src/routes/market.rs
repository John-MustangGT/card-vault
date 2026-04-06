use axum::{
    extract::{Query, State},
    response::Html,
    Json,
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct MarketQuery {
    pub set: Option<String>,
}

pub async fn market_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MarketQuery>,
) -> Html<String> {
    // ── Import history ───────────────────────────────────────────────────────
    let import_rows = sqlx::query(
        "SELECT id, filename, cards_processed, imported_at, duration_secs
         FROM scryfall_bulk_imports ORDER BY id DESC LIMIT 10",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let imports: Vec<serde_json::Value> = import_rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get::<i64, _>("id"),
                "filename": r.get::<String, _>("filename"),
                "cards_processed": r.get::<i64, _>("cards_processed"),
                "imported_at": r.get::<i64, _>("imported_at"),
                "duration_secs": r.get::<f64, _>("duration_secs"),
            })
        })
        .collect();

    // IDs of last two imports (for delta calculation)
    let last_ids: Vec<i64> = import_rows
        .iter()
        .take(2)
        .map(|r| r.get::<i64, _>("id"))
        .collect();

    // ── Collection movers (cards in inventory_lots that have price data) ─────
    let collection_movers: Vec<serde_json::Value> = if last_ids.len() >= 2 {
        let (new_id, old_id) = (last_ids[0], last_ids[1]);
        let rows = sqlx::query(
            r#"
            SELECT
                sc.name,
                sc.set_code,
                sc.scryfall_id,
                il.foil,
                SUM(il.quantity) as qty,
                bp_new.price_usd  AS price_new,
                bp_old.price_usd  AS price_old
            FROM inventory_lots il
            JOIN scryfall_cards sc ON sc.scryfall_id = il.scryfall_id
            LEFT JOIN bulk_prices bp_new ON bp_new.scryfall_id = il.scryfall_id AND bp_new.import_id = ?
            LEFT JOIN bulk_prices bp_old ON bp_old.scryfall_id = il.scryfall_id AND bp_old.import_id = ?
            WHERE bp_new.price_usd IS NOT NULL AND bp_old.price_usd IS NOT NULL
              AND bp_new.price_usd != bp_old.price_usd
            GROUP BY sc.scryfall_id, il.foil
            ORDER BY (bp_new.price_usd - bp_old.price_usd) DESC
            LIMIT 30
            "#,
        )
        .bind(new_id)
        .bind(old_id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();

        rows.iter()
            .map(|r| {
                let new_p: f64 = r.get("price_new");
                let old_p: f64 = r.get("price_old");
                let delta = new_p - old_p;
                let pct = if old_p > 0.0 { delta / old_p * 100.0 } else { 0.0 };
                serde_json::json!({
                    "name": r.get::<String, _>("name"),
                    "set_code": r.get::<String, _>("set_code"),
                    "scryfall_id": r.get::<String, _>("scryfall_id"),
                    "foil": r.get::<String, _>("foil"),
                    "qty": r.get::<i64, _>("qty"),
                    "price_new": new_p,
                    "price_old": old_p,
                    "delta": delta,
                    "pct": pct,
                })
            })
            .collect()
    } else {
        vec![]
    };

    // ── Top risers / fallers (all cards, optionally filtered by set) ─────────
    let (risers, fallers) = if last_ids.len() >= 2 {
        let (new_id, old_id) = (last_ids[0], last_ids[1]);

        let set_filter = params
            .set
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("");

        // Build query with optional set filter
        let base_sql = r#"
            SELECT
                bc.name,
                bc.set_code,
                bc.scryfall_id,
                bp_new.price_usd  AS price_new,
                bp_old.price_usd  AS price_old,
                (bp_new.price_usd - bp_old.price_usd) AS delta,
                CASE WHEN bp_old.price_usd > 0
                     THEN (bp_new.price_usd - bp_old.price_usd) / bp_old.price_usd * 100
                     ELSE 0 END AS pct
            FROM bulk_prices bp_new
            JOIN bulk_prices bp_old ON bp_old.scryfall_id = bp_new.scryfall_id AND bp_old.import_id = ?
            JOIN scryfall_bulk_cards bc ON bc.scryfall_id = bp_new.scryfall_id
            WHERE bp_new.import_id = ?
              AND bp_new.price_usd IS NOT NULL
              AND bp_old.price_usd IS NOT NULL
              AND bp_new.price_usd >= 0.25
              AND bp_new.price_usd != bp_old.price_usd
        "#;

        let riser_rows = if set_filter.is_empty() {
            sqlx::query(&format!("{} ORDER BY delta DESC LIMIT 25", base_sql))
                .bind(old_id)
                .bind(new_id)
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default()
        } else {
            sqlx::query(&format!(
                "{} AND bc.set_code = ? ORDER BY delta DESC LIMIT 25",
                base_sql
            ))
            .bind(old_id)
            .bind(new_id)
            .bind(set_filter)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
        };

        let faller_rows = if set_filter.is_empty() {
            sqlx::query(&format!("{} ORDER BY delta ASC LIMIT 25", base_sql))
                .bind(old_id)
                .bind(new_id)
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default()
        } else {
            sqlx::query(&format!(
                "{} AND bc.set_code = ? ORDER BY delta ASC LIMIT 25",
                base_sql
            ))
            .bind(old_id)
            .bind(new_id)
            .bind(set_filter)
            .fetch_all(&state.pool)
            .await
            .unwrap_or_default()
        };

        let map_row = |r: &sqlx::sqlite::SqliteRow| {
            serde_json::json!({
                "name": r.get::<String, _>("name"),
                "set_code": r.get::<String, _>("set_code"),
                "scryfall_id": r.get::<String, _>("scryfall_id"),
                "price_new": r.get::<f64, _>("price_new"),
                "price_old": r.get::<f64, _>("price_old"),
                "delta": r.get::<f64, _>("delta"),
                "pct": r.get::<f64, _>("pct"),
            })
        };

        (
            riser_rows.iter().map(map_row).collect::<Vec<_>>(),
            faller_rows.iter().map(map_row).collect::<Vec<_>>(),
        )
    } else {
        (vec![], vec![])
    };

    // ── Available sets (for filter dropdown) ────────────────────────────────
    let set_rows = sqlx::query(
        "SELECT DISTINCT set_code, set_name FROM scryfall_bulk_cards WHERE set_code != '' ORDER BY set_code ASC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let sets: Vec<serde_json::Value> = set_rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "set_code": r.get::<String, _>("set_code"),
                "set_name": r.get::<String, _>("set_name"),
            })
        })
        .collect();

    let tmpl = state
        .env
        .get_template("market.html")
        .expect("market.html missing");
    let ctx = minijinja::context! {
        imports => imports,
        collection_movers => collection_movers,
        risers => risers,
        fallers => fallers,
        sets => sets,
        filter_set => params.set.unwrap_or_default(),
    };
    Html(tmpl.render(ctx).expect("template render failed"))
}

/// POST /market/import — trigger immediate re-scan and import of data/*.json.gz
pub async fn trigger_import(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let pool = state.pool.clone();
    let data_dir = state.config.data_dir.clone();

    tokio::spawn(async move {
        match crate::db::bulk::run_import(&pool, &data_dir).await {
            Ok(n) => tracing::info!("manual import: {} new files", n),
            Err(e) => tracing::warn!("manual import error: {}", e),
        }
    });

    Json(serde_json::json!({ "ok": true, "message": "Import started in background" }))
}

/// GET /market/search?q=&set=&rarity=&price_min=&price_max= — JSON price browse
#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub q: Option<String>,
    pub set: Option<String>,
    pub rarity: Option<String>,
    pub price_min: Option<f64>,
    pub price_max: Option<f64>,
}

pub async fn search_prices(
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchQuery>,
) -> Json<serde_json::Value> {
    let q      = params.q.as_deref().unwrap_or("").trim().to_string();
    let set    = params.set.as_deref().unwrap_or("").trim().to_string();
    let rarity = params.rarity.as_deref().unwrap_or("").trim().to_string();
    // -1.0 signals "no filter" — condition `? < 0` short-circuits the price clause
    let price_min = params.price_min.unwrap_or(-1.0);
    let price_max = params.price_max.unwrap_or(-1.0);

    // Need at least a name fragment, set, or rarity to search
    if q.is_empty() && set.is_empty() && rarity.is_empty() {
        return Json(serde_json::json!({ "cards": [] }));
    }

    let like_q = format!("%{}%", q);

    let rows = sqlx::query(
        r#"
        SELECT bc.scryfall_id, bc.name, bc.set_code, bc.set_name, bc.rarity,
               bc.type_line, bc.collector_number,
               bp.price_usd, bp.price_usd_foil, bp.price_usd_etched
        FROM scryfall_bulk_cards bc
        LEFT JOIN bulk_prices bp
            ON bp.scryfall_id = bc.scryfall_id
            AND bp.import_id = (SELECT MAX(id) FROM scryfall_bulk_imports)
        WHERE (? = '' OR bc.name LIKE ?)
          AND (? = '' OR bc.set_code = ?)
          AND (? = '' OR bc.rarity = ?)
          AND (? < 0 OR bp.price_usd >= ?)
          AND (? < 0 OR bp.price_usd <= ?)
        ORDER BY bc.name, bc.set_code, CAST(bc.collector_number AS INTEGER)
        LIMIT 200
        "#,
    )
    .bind(&q).bind(&like_q)
    .bind(&set).bind(&set)
    .bind(&rarity).bind(&rarity)
    .bind(price_min).bind(price_min)
    .bind(price_max).bind(price_max)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let cards: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "scryfall_id":   r.get::<String, _>("scryfall_id"),
                "name":          r.get::<String, _>("name"),
                "set_code":      r.get::<String, _>("set_code"),
                "set_name":      r.get::<String, _>("set_name"),
                "rarity":        r.get::<String, _>("rarity"),
                "type_line":     r.get::<String, _>("type_line"),
                "collector_number": r.get::<String, _>("collector_number"),
                "price_usd":     r.get::<Option<f64>, _>("price_usd"),
                "price_usd_foil":r.get::<Option<f64>, _>("price_usd_foil"),
                "price_usd_etched": r.get::<Option<f64>, _>("price_usd_etched"),
            })
        })
        .collect();

    Json(serde_json::json!({ "cards": cards }))
}

/// POST /market/clear — truncate all bulk market tables so files get re-imported
pub async fn clear_market(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let r1 = sqlx::query("DELETE FROM bulk_prices").execute(&state.pool).await;
    let r2 = sqlx::query("DELETE FROM scryfall_bulk_cards").execute(&state.pool).await;
    let r3 = sqlx::query("DELETE FROM scryfall_bulk_imports").execute(&state.pool).await;

    if r1.is_err() || r2.is_err() || r3.is_err() {
        return Json(serde_json::json!({ "ok": false, "error": "Delete failed" }));
    }

    tracing::info!("market data cleared by user");
    Json(serde_json::json!({ "ok": true }))
}
