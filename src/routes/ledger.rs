use axum::{
    extract::{Path, State},
    response::{Html, Redirect},
    Json,
};
use chrono::Local;
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

pub async fn ledger_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let rows = sqlx::query(
        "SELECT id, entry_date, category, description, amount, notes, created_at
         FROM ledger_entries ORDER BY entry_date DESC, id DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let entries: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id":          r.get::<i64, _>("id"),
                "entry_date":  r.get::<String, _>("entry_date"),
                "category":    r.get::<String, _>("category"),
                "description": r.get::<String, _>("description"),
                "amount":      r.get::<f64, _>("amount"),
                "notes":       r.get::<Option<String>, _>("notes").unwrap_or_default(),
            })
        })
        .collect();

    // Totals by category
    let cat_rows = sqlx::query(
        "SELECT category, SUM(amount) as total FROM ledger_entries GROUP BY category ORDER BY total DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let by_category: Vec<serde_json::Value> = cat_rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "category": r.get::<String, _>("category"),
                "total":    r.get::<f64, _>("total"),
            })
        })
        .collect();

    let grand_total: f64 = entries.iter()
        .filter_map(|e| e["amount"].as_f64())
        .sum();

    let today = Local::now().format("%Y-%m-%d").to_string();

    let tmpl = state.env.get_template("ledger.html").expect("ledger.html missing");
    let ctx = minijinja::context! {
        entries      => entries,
        by_category  => by_category,
        grand_total  => grand_total,
        today        => today,
    };
    Html(tmpl.render(ctx).expect("template render failed"))
}

#[derive(Deserialize)]
pub struct LedgerForm {
    pub entry_date:  String,
    pub category:    String,
    pub description: String,
    pub amount:      f64,
    pub notes:       Option<String>,
}

pub async fn create_entry(
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<LedgerForm>,
) -> Redirect {
    let now = unix_now();
    let _ = sqlx::query(
        "INSERT INTO ledger_entries (entry_date, category, description, amount, notes, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&form.entry_date)
    .bind(&form.category)
    .bind(&form.description)
    .bind(form.amount)
    .bind(form.notes.as_deref().filter(|s| !s.is_empty()))
    .bind(now)
    .execute(&state.pool)
    .await;

    Redirect::to("/ledger")
}

pub async fn delete_entry(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    let _ = sqlx::query("DELETE FROM ledger_entries WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await;
    Json(serde_json::json!({ "ok": true }))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
