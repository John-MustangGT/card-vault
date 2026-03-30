use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect, Response},
    Json,
};
use axum::http::header;
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

pub async fn sales_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let rows = sqlx::query(
        r#"SELECT t.id, t.buyer_name, t.platform, t.platform_order_id, t.status,
                  t.shipping_cost, t.sold_at,
                  COUNT(ti.id) as item_count,
                  COALESCE(SUM(ti.sale_price * ti.quantity), 0.0) as subtotal
           FROM transactions t
           LEFT JOIN transaction_items ti ON ti.transaction_id = t.id
           GROUP BY t.id ORDER BY t.sold_at DESC"#
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let transactions: Vec<serde_json::Value> = rows.iter().map(|r| {
        let subtotal: f64 = r.get("subtotal");
        let shipping: f64 = r.get::<Option<f64>, _>("shipping_cost").unwrap_or(0.0);
        serde_json::json!({
            "id": r.get::<i64, _>("id"),
            "buyer_name": r.get::<Option<String>, _>("buyer_name").unwrap_or_default(),
            "platform": r.get::<Option<String>, _>("platform").unwrap_or_default(),
            "platform_order_id": r.get::<Option<String>, _>("platform_order_id").unwrap_or_default(),
            "status": r.get::<String, _>("status"),
            "item_count": r.get::<i64, _>("item_count"),
            "total": subtotal + shipping,
            "sold_at": r.get::<i64, _>("sold_at"),
        })
    }).collect();

    let tmpl = state.env.get_template("sales.html").expect("sales.html missing");
    let ctx = minijinja::context! { transactions => transactions };
    Html(tmpl.render(ctx).expect("template render failed"))
}

pub async fn new_sale_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let tmpl = state.env.get_template("sales_new.html").expect("sales_new.html missing");
    Html(tmpl.render(minijinja::context!()).expect("template render failed"))
}

#[derive(Deserialize)]
pub struct SaleItem {
    pub description: String,
    pub quantity: i64,
    pub unit_price: f64,
}

#[derive(Deserialize)]
pub struct CreateSaleRequest {
    pub buyer_name: String,
    pub buyer_email: Option<String>,
    pub buyer_address: Option<String>,
    pub buyer_city: Option<String>,
    pub buyer_state: Option<String>,
    pub buyer_zip: Option<String>,
    pub platform: Option<String>,
    pub platform_order_id: Option<String>,
    pub shipping_cost: Option<f64>,
    pub items: Vec<SaleItem>,
}

pub async fn create_sale(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSaleRequest>,
) -> Json<serde_json::Value> {
    let now = unix_now();

    let result = sqlx::query(
        "INSERT INTO transactions
         (buyer_name, buyer_email, buyer_address, buyer_city, buyer_state, buyer_zip,
          platform, platform_order_id, shipping_cost, status, sold_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)"
    )
    .bind(&req.buyer_name)
    .bind(&req.buyer_email)
    .bind(&req.buyer_address)
    .bind(&req.buyer_city)
    .bind(&req.buyer_state)
    .bind(&req.buyer_zip)
    .bind(&req.platform)
    .bind(&req.platform_order_id)
    .bind(req.shipping_cost)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await;

    let txn_id = match result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    };

    for item in &req.items {
        if item.description.trim().is_empty() {
            continue;
        }
        let _ = sqlx::query(
            "INSERT INTO transaction_items (transaction_id, description, quantity, sale_price, currency)
             VALUES (?, ?, ?, ?, 'USD')"
        )
        .bind(txn_id)
        .bind(&item.description)
        .bind(item.quantity)
        .bind(item.unit_price)
        .execute(&state.pool)
        .await;
    }

    Json(serde_json::json!({ "ok": true, "id": txn_id }))
}

pub async fn sale_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Html<String> {
    let txn = sqlx::query(
        "SELECT id, buyer_name, buyer_email, buyer_address, buyer_city, buyer_state, buyer_zip,
                platform, platform_order_id, shipping_cost, status, tracking_number, sold_at
         FROM transactions WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let txn = match txn {
        None => return Html("<p>Transaction not found.</p>".into()),
        Some(r) => serde_json::json!({
            "id": r.get::<i64, _>("id"),
            "buyer_name": r.get::<Option<String>, _>("buyer_name").unwrap_or_default(),
            "buyer_email": r.get::<Option<String>, _>("buyer_email").unwrap_or_default(),
            "buyer_address": r.get::<Option<String>, _>("buyer_address").unwrap_or_default(),
            "buyer_city": r.get::<Option<String>, _>("buyer_city").unwrap_or_default(),
            "buyer_state": r.get::<Option<String>, _>("buyer_state").unwrap_or_default(),
            "buyer_zip": r.get::<Option<String>, _>("buyer_zip").unwrap_or_default(),
            "platform": r.get::<Option<String>, _>("platform").unwrap_or_default(),
            "platform_order_id": r.get::<Option<String>, _>("platform_order_id").unwrap_or_default(),
            "shipping_cost": r.get::<Option<f64>, _>("shipping_cost").unwrap_or(0.0),
            "status": r.get::<String, _>("status"),
            "tracking_number": r.get::<Option<String>, _>("tracking_number").unwrap_or_default(),
            "sold_at": r.get::<i64, _>("sold_at"),
        }),
    };

    let item_rows = sqlx::query(
        "SELECT id, description, quantity, sale_price, currency FROM transaction_items WHERE transaction_id = ?"
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let items: Vec<serde_json::Value> = item_rows.iter().map(|r| {
        let qty: i64 = r.get("quantity");
        let price: f64 = r.get("sale_price");
        serde_json::json!({
            "id": r.get::<i64, _>("id"),
            "description": r.get::<Option<String>, _>("description").unwrap_or_default(),
            "quantity": qty,
            "sale_price": price,
            "line_total": qty as f64 * price,
            "currency": r.get::<String, _>("currency"),
        })
    }).collect();

    let tmpl = state.env.get_template("sales_detail.html").expect("sales_detail.html missing");
    let ctx = minijinja::context! { txn => txn, items => items };
    Html(tmpl.render(ctx).expect("template render failed"))
}

pub async fn sale_label(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Response {
    let row = sqlx::query(
        "SELECT buyer_name, buyer_address, buyer_city, buyer_state, buyer_zip, platform_order_id
         FROM transactions WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    let row = match row {
        None => return (axum::http::StatusCode::NOT_FOUND, "Not found").into_response(),
        Some(r) => r,
    };

    let name    = row.get::<Option<String>, _>("buyer_name").unwrap_or_default();
    let addr    = row.get::<Option<String>, _>("buyer_address").unwrap_or_default();
    let city    = row.get::<Option<String>, _>("buyer_city").unwrap_or_default();
    let state_s = row.get::<Option<String>, _>("buyer_state").unwrap_or_default();
    let zip     = row.get::<Option<String>, _>("buyer_zip").unwrap_or_default();
    let oid     = row.get::<Option<String>, _>("platform_order_id")
        .unwrap_or_else(|| format!("#{}", id));

    let csz = format!("{} {} {}", city, state_s, zip).trim().to_string();
    let content = format!("{}|{}|{}|{}\n", name, addr, csz, oid);
    let disposition = format!("attachment; filename=\"label_{}.txt\"", id);

    let mut resp = content.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        "text/plain".parse().unwrap(),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        disposition.parse().unwrap(),
    );
    resp
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
