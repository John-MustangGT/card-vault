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
    // ── Expenses ─────────────────────────────────────────────────────────────
    let expense_rows = sqlx::query(
        "SELECT id, entry_date, category, description, amount, notes
         FROM ledger_entries ORDER BY entry_date DESC, id DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let entries: Vec<serde_json::Value> = expense_rows
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

    let by_category: Vec<serde_json::Value> = sqlx::query(
        "SELECT category, SUM(amount) as total FROM ledger_entries GROUP BY category ORDER BY total DESC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| serde_json::json!({
        "category": r.get::<String, _>("category"),
        "total":    r.get::<f64, _>("total"),
    }))
    .collect();

    let total_expenses: f64 = entries.iter().filter_map(|e| e["amount"].as_f64()).sum();

    // ── Revenue from invoices ─────────────────────────────────────────────────
    let invoice_rows = sqlx::query(
        r#"SELECT t.id, t.invoice_id, t.buyer_name, t.platform, t.sold_at, t.shipping_charged,
                  COALESCE(SUM(ti.sale_price * ti.quantity), 0.0) as subtotal,
                  COALESCE(t.shipping_cost, 0.0) as shipping_cost
           FROM transactions t
           LEFT JOIN transaction_items ti ON ti.transaction_id = t.id
           GROUP BY t.id
           ORDER BY t.sold_at DESC"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let invoices: Vec<serde_json::Value> = invoice_rows
        .iter()
        .map(|r| {
            let subtotal: f64 = r.get("subtotal");
            let shipping: f64 = r.get("shipping_cost");
            let charged:  bool = r.get::<i64, _>("shipping_charged") != 0;
            let total = subtotal + if charged { shipping } else { 0.0 };
            let invoice_id = r.get::<Option<String>, _>("invoice_id")
                .unwrap_or_else(|| format!("{}", r.get::<i64, _>("id")));
            serde_json::json!({
                "id":          r.get::<i64, _>("id"),
                "invoice_id":  invoice_id,
                "buyer_name":  r.get::<Option<String>, _>("buyer_name").unwrap_or_default(),
                "platform":    r.get::<Option<String>, _>("platform").unwrap_or_default(),
                "sold_at":     r.get::<i64, _>("sold_at"),
                "subtotal":    subtotal,
                "shipping":    if charged { shipping } else { 0.0 },
                "total":       total,
            })
        })
        .collect();

    let total_revenue: f64 = invoices.iter().filter_map(|i| i["total"].as_f64()).sum();
    let net: f64 = total_revenue - total_expenses;

    let today = Local::now().format("%Y-%m-%d").to_string();

    let tmpl = state.env.get_template("ledger.html").expect("ledger.html missing");
    let ctx = minijinja::context! {
        entries         => entries,
        by_category     => by_category,
        total_expenses  => total_expenses,
        invoices        => invoices,
        total_revenue   => total_revenue,
        net             => net,
        today           => today,
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
