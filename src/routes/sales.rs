use axum::{
    extract::{Path, Query, State},
    response::{Html, IntoResponse, Response},
    Json,
};
use axum::http::header;
use chrono::Local;
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn gen_invoice_id() -> String {
    let now = Local::now();
    let mins = now.format("%H").to_string().parse::<u32>().unwrap_or(0) * 60
        + now.format("%M").to_string().parse::<u32>().unwrap_or(0);
    format!("{}-{}", now.format("%Y%m%d"), mins)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ── Inventory autocomplete ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AutocompleteQuery {
    pub q: Option<String>,
}

pub async fn inventory_autocomplete(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AutocompleteQuery>,
) -> Json<serde_json::Value> {
    let q = params.q.as_deref().unwrap_or("").trim().to_string();
    if q.len() < 2 {
        return Json(serde_json::json!({ "results": [] }));
    }
    let like_q = format!("%{}%", q);

    // Card lots from inventory
    let card_rows = sqlx::query(
        r#"SELECT sc.name, sc.set_code, sc.set_name, il.condition, il.foil,
                  SUM(il.quantity) as qty
           FROM inventory_lots il
           JOIN scryfall_cards sc ON sc.scryfall_id = il.scryfall_id
           WHERE sc.name LIKE ?
           GROUP BY sc.scryfall_id, il.condition, il.foil
           ORDER BY sc.name, il.condition
           LIMIT 15"#,
    )
    .bind(&like_q)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    // Sealed products
    let sealed_rows = sqlx::query(
        "SELECT id, name, set_code, set_name, product_type, language, quantity
         FROM sealed_products WHERE name LIKE ? AND quantity > 0 ORDER BY name LIMIT 10",
    )
    .bind(&like_q)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut results: Vec<serde_json::Value> = card_rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "kind":      "card",
                "name":      r.get::<String, _>("name"),
                "set_code":  r.get::<String, _>("set_code"),
                "set_name":  r.get::<String, _>("set_name"),
                "condition": r.get::<String, _>("condition"),
                "foil":      r.get::<String, _>("foil"),
                "qty":       r.get::<i64, _>("qty"),
            })
        })
        .collect();

    for r in &sealed_rows {
        results.push(serde_json::json!({
            "kind":         "sealed",
            "sealed_id":    r.get::<i64, _>("id"),
            "name":         r.get::<String, _>("name"),
            "set_code":     r.get::<String, _>("set_code"),
            "set_name":     r.get::<String, _>("set_name"),
            "product_type": r.get::<String, _>("product_type"),
            "language":     r.get::<String, _>("language"),
            "qty":          r.get::<i64, _>("quantity"),
        }));
    }

    Json(serde_json::json!({ "results": results }))
}

// ── Sales list ────────────────────────────────────────────────────────────────

pub async fn sales_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let rows = sqlx::query(
        r#"SELECT t.id, t.invoice_id, t.buyer_name, t.platform, t.platform_order_id,
                  t.status, t.shipping_cost, t.shipping_charged, t.sold_at,
                  COUNT(ti.id) as item_count,
                  COALESCE(SUM(ti.sale_price * ti.quantity), 0.0) as subtotal
           FROM transactions t
           LEFT JOIN transaction_items ti ON ti.transaction_id = t.id
           GROUP BY t.id ORDER BY t.sold_at DESC"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let transactions: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let subtotal: f64 = r.get("subtotal");
            let shipping: f64 = r.get::<Option<f64>, _>("shipping_cost").unwrap_or(0.0);
            let charged: bool = r.get::<i64, _>("shipping_charged") != 0;
            let invoice_id = r
                .get::<Option<String>, _>("invoice_id")
                .unwrap_or_else(|| format!("{}", r.get::<i64, _>("id")));
            serde_json::json!({
                "id": r.get::<i64, _>("id"),
                "invoice_id": invoice_id,
                "buyer_name": r.get::<Option<String>, _>("buyer_name").unwrap_or_default(),
                "platform": r.get::<Option<String>, _>("platform").unwrap_or_default(),
                "platform_order_id": r.get::<Option<String>, _>("platform_order_id").unwrap_or_default(),
                "status": r.get::<String, _>("status"),
                "item_count": r.get::<i64, _>("item_count"),
                "total": subtotal + if charged { shipping } else { 0.0 },
                "sold_at": r.get::<i64, _>("sold_at"),
            })
        })
        .collect();

    let tmpl = state.env.get_template("sales.html").expect("sales.html missing");
    let ctx = minijinja::context! { transactions => transactions };
    Html(tmpl.render(ctx).expect("template render failed"))
}

// ── New sale form ─────────────────────────────────────────────────────────────

pub async fn new_sale_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let tmpl = state
        .env
        .get_template("sales_form.html")
        .expect("sales_form.html missing");
    Html(
        tmpl.render(minijinja::context! {
            mode   => "new",
            prefill => "",
        })
        .expect("template render failed"),
    )
}

// ── Edit sale form ────────────────────────────────────────────────────────────

pub async fn edit_sale_page(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Html<String> {
    let txn = sqlx::query(
        "SELECT id, invoice_id, buyer_name, buyer_email, buyer_address,
                buyer_city, buyer_state, buyer_zip,
                platform, platform_order_id, shipping_cost, shipping_charged,
                tracking_number, status
         FROM transactions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let txn = match txn {
        None => return Html("<p>Not found.</p>".into()),
        Some(r) => r,
    };

    let item_rows = sqlx::query(
        "SELECT description, set_code, condition, quantity, sale_price
         FROM transaction_items WHERE transaction_id = ?",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let prefill = serde_json::json!({
        "id": id,
        "invoice_id": txn.get::<Option<String>,_>("invoice_id").unwrap_or_default(),
        "buyer_name": txn.get::<Option<String>,_>("buyer_name").unwrap_or_default(),
        "buyer_email": txn.get::<Option<String>,_>("buyer_email").unwrap_or_default(),
        "buyer_address": txn.get::<Option<String>,_>("buyer_address").unwrap_or_default(),
        "buyer_city": txn.get::<Option<String>,_>("buyer_city").unwrap_or_default(),
        "buyer_state": txn.get::<Option<String>,_>("buyer_state").unwrap_or_default(),
        "buyer_zip": txn.get::<Option<String>,_>("buyer_zip").unwrap_or_default(),
        "platform": txn.get::<Option<String>,_>("platform").unwrap_or_default(),
        "platform_order_id": txn.get::<Option<String>,_>("platform_order_id").unwrap_or_default(),
        "shipping_cost": txn.get::<Option<f64>,_>("shipping_cost").unwrap_or(0.0),
        "shipping_charged": txn.get::<i64,_>("shipping_charged") != 0,
        "tracking_number": txn.get::<Option<String>,_>("tracking_number").unwrap_or_default(),
        "items": item_rows.iter().map(|r| serde_json::json!({
            "description": r.get::<Option<String>,_>("description").unwrap_or_default(),
            "set_code":    r.get::<Option<String>,_>("set_code").unwrap_or_default(),
            "condition":   r.get::<Option<String>,_>("condition").unwrap_or_default(),
            "quantity":    r.get::<i64,_>("quantity"),
            "unit_price":  r.get::<f64,_>("sale_price"),
        })).collect::<Vec<_>>(),
    });

    let prefill_json = serde_json::to_string(&prefill).unwrap_or_default();

    let tmpl = state
        .env
        .get_template("sales_form.html")
        .expect("sales_form.html missing");
    Html(
        tmpl.render(minijinja::context! {
            mode    => "edit",
            prefill => prefill_json,
        })
        .expect("template render failed"),
    )
}

// ── Create / Update ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SaleItem {
    pub description: String,
    pub set_code:    Option<String>,
    pub condition:   Option<String>,
    pub quantity:    i64,
    pub unit_price:  f64,
    pub sealed_id:   Option<i64>,
}

#[derive(Deserialize)]
pub struct CreateSaleRequest {
    pub buyer_name:        String,
    pub buyer_email:       Option<String>,
    pub buyer_address:     Option<String>,
    pub buyer_city:        Option<String>,
    pub buyer_state:       Option<String>,
    pub buyer_zip:         Option<String>,
    pub platform:          Option<String>,
    pub platform_order_id: Option<String>,
    pub shipping_cost:     Option<f64>,
    pub shipping_charged:  Option<bool>,
    pub tracking_number:   Option<String>,
    pub items:             Vec<SaleItem>,
}

pub async fn create_sale(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSaleRequest>,
) -> Json<serde_json::Value> {
    let now = unix_now();
    let invoice_id = gen_invoice_id();
    let shipping_charged = req.shipping_charged.unwrap_or(true) as i64;

    let result = sqlx::query(
        "INSERT INTO transactions
         (invoice_id, buyer_name, buyer_email, buyer_address, buyer_city, buyer_state, buyer_zip,
          platform, platform_order_id, shipping_cost, shipping_charged, tracking_number,
          status, sold_at, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?)",
    )
    .bind(&invoice_id)
    .bind(&req.buyer_name)
    .bind(&req.buyer_email)
    .bind(&req.buyer_address)
    .bind(&req.buyer_city)
    .bind(&req.buyer_state)
    .bind(&req.buyer_zip)
    .bind(&req.platform)
    .bind(&req.platform_order_id)
    .bind(req.shipping_cost)
    .bind(shipping_charged)
    .bind(&req.tracking_number)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await;

    let txn_id = match result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => return Json(serde_json::json!({ "ok": false, "error": e.to_string() })),
    };

    insert_items(&state.pool, txn_id, &req.items).await;
    Json(serde_json::json!({ "ok": true, "id": txn_id }))
}

pub async fn update_sale(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(req): Json<CreateSaleRequest>,
) -> Json<serde_json::Value> {
    let shipping_charged = req.shipping_charged.unwrap_or(true) as i64;

    let result = sqlx::query(
        "UPDATE transactions SET
           buyer_name=?, buyer_email=?, buyer_address=?, buyer_city=?, buyer_state=?, buyer_zip=?,
           platform=?, platform_order_id=?, shipping_cost=?, shipping_charged=?, tracking_number=?
         WHERE id=?",
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
    .bind(shipping_charged)
    .bind(&req.tracking_number)
    .bind(id)
    .execute(&state.pool)
    .await;

    if let Err(e) = result {
        return Json(serde_json::json!({ "ok": false, "error": e.to_string() }));
    }

    // Restore inventory for old items, then replace
    restore_inventory(&state.pool, id).await;
    let _ = sqlx::query("DELETE FROM transaction_items WHERE transaction_id=?")
        .bind(id)
        .execute(&state.pool)
        .await;

    insert_items(&state.pool, id, &req.items).await;
    Json(serde_json::json!({ "ok": true, "id": id }))
}

async fn insert_items(pool: &sqlx::SqlitePool, txn_id: i64, items: &[SaleItem]) {
    let now = unix_now();
    for item in items {
        if item.description.trim().is_empty() {
            continue;
        }
        let set_code = item.set_code.as_deref().filter(|s| !s.is_empty()).unwrap_or("");
        let condition = item.condition.as_deref().filter(|s| !s.is_empty()).unwrap_or("");

        // Try to find and deduct from inventory lot (if set+condition are known)
        let lot_id = if !set_code.is_empty() && !condition.is_empty() && item.sealed_id.is_none() {
            deduct_lot(pool, &item.description, set_code, condition, item.quantity, now).await
        } else {
            None
        };

        // Deduct from sealed product if sealed_id provided
        if let Some(sid) = item.sealed_id {
            let _ = sqlx::query(
                "UPDATE sealed_products SET quantity = MAX(0, quantity - ?), updated_at = ? WHERE id = ?",
            )
            .bind(item.quantity)
            .bind(now)
            .bind(sid)
            .execute(pool)
            .await;
        }

        let _ = sqlx::query(
            "INSERT INTO transaction_items
             (transaction_id, description, set_code, condition, quantity, sale_price, currency, lot_id, sealed_id)
             VALUES (?, ?, ?, ?, ?, ?, 'USD', ?, ?)",
        )
        .bind(txn_id)
        .bind(&item.description)
        .bind(if set_code.is_empty() { None } else { Some(set_code) })
        .bind(if condition.is_empty() { None } else { Some(condition) })
        .bind(item.quantity)
        .bind(item.unit_price)
        .bind(lot_id)
        .bind(item.sealed_id)
        .execute(pool)
        .await;
    }
}

/// Parse foil suffix from description; return (base_name, foil_type).
fn parse_desc_foil(desc: &str) -> (&str, &str) {
    let d = desc.trim();
    if d.ends_with(" (foil)")   { return (&d[..d.len()-7],  "foil"); }
    if d.ends_with(" (etched)") { return (&d[..d.len()-9], "etched"); }
    (d, "normal")
}

/// Find the best matching lot for name+set+condition+foil, deduct qty, return lot_id.
async fn deduct_lot(
    pool: &sqlx::SqlitePool,
    description: &str,
    set_code: &str,
    condition: &str,
    quantity: i64,
    now: i64,
) -> Option<i64> {
    let (name, foil) = parse_desc_foil(description);
    let row = sqlx::query(
        "SELECT il.id FROM inventory_lots il
         JOIN scryfall_cards sc ON sc.scryfall_id = il.scryfall_id
         WHERE sc.name = ? AND LOWER(sc.set_code) = LOWER(?) AND il.condition = ? AND il.foil = ?
         LIMIT 1",
    )
    .bind(name)
    .bind(set_code)
    .bind(condition)
    .bind(foil)
    .fetch_optional(pool)
    .await
    .ok()??;

    let lot_id: i64 = row.get("id");
    let _ = sqlx::query(
        "UPDATE inventory_lots SET quantity = MAX(0, quantity - ?), updated_at = ? WHERE id = ?",
    )
    .bind(quantity)
    .bind(now)
    .bind(lot_id)
    .execute(pool)
    .await;

    Some(lot_id)
}

/// Restore inventory quantities for all linked lots/sealed on a transaction being replaced.
async fn restore_inventory(pool: &sqlx::SqlitePool, txn_id: i64) {
    let now = unix_now();
    let rows = sqlx::query(
        "SELECT lot_id, sealed_id, quantity FROM transaction_items WHERE transaction_id = ?",
    )
    .bind(txn_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for row in rows {
        let qty: i64 = row.get("quantity");
        if let Some(lot_id) = row.get::<Option<i64>, _>("lot_id") {
            let _ = sqlx::query(
                "UPDATE inventory_lots SET quantity = quantity + ?, updated_at = ? WHERE id = ?",
            )
            .bind(qty).bind(now).bind(lot_id)
            .execute(pool).await;
        }
        if let Some(sid) = row.get::<Option<i64>, _>("sealed_id") {
            let _ = sqlx::query(
                "UPDATE sealed_products SET quantity = quantity + ?, updated_at = ? WHERE id = ?",
            )
            .bind(qty).bind(now).bind(sid)
            .execute(pool).await;
        }
    }
}

// ── Sale detail / invoice ─────────────────────────────────────────────────────

pub async fn sale_detail(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Html<String> {
    let txn = sqlx::query(
        "SELECT id, invoice_id, buyer_name, buyer_email, buyer_address,
                buyer_city, buyer_state, buyer_zip,
                platform, platform_order_id, shipping_cost, shipping_charged,
                tracking_number, status, sold_at
         FROM transactions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let txn = match txn {
        None => return Html("<p>Transaction not found.</p>".into()),
        Some(r) => {
            let invoice_id = r
                .get::<Option<String>, _>("invoice_id")
                .unwrap_or_else(|| format!("{}", r.get::<i64, _>("id")));
            serde_json::json!({
                "id": r.get::<i64, _>("id"),
                "invoice_id": invoice_id,
                "buyer_name": r.get::<Option<String>, _>("buyer_name").unwrap_or_default(),
                "buyer_email": r.get::<Option<String>, _>("buyer_email").unwrap_or_default(),
                "buyer_address": r.get::<Option<String>, _>("buyer_address").unwrap_or_default(),
                "buyer_city": r.get::<Option<String>, _>("buyer_city").unwrap_or_default(),
                "buyer_state": r.get::<Option<String>, _>("buyer_state").unwrap_or_default(),
                "buyer_zip": r.get::<Option<String>, _>("buyer_zip").unwrap_or_default(),
                "platform": r.get::<Option<String>, _>("platform").unwrap_or_default(),
                "platform_order_id": r.get::<Option<String>, _>("platform_order_id").unwrap_or_default(),
                "shipping_cost": r.get::<Option<f64>, _>("shipping_cost").unwrap_or(0.0),
                "shipping_charged": r.get::<i64, _>("shipping_charged") != 0,
                "tracking_number": r.get::<Option<String>, _>("tracking_number").unwrap_or_default(),
                "status": r.get::<String, _>("status"),
                "sold_at": r.get::<i64, _>("sold_at"),
            })
        }
    };

    let item_rows = sqlx::query(
        "SELECT id, description, set_code, condition, quantity, sale_price, currency
         FROM transaction_items WHERE transaction_id = ?",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let items: Vec<serde_json::Value> = item_rows
        .iter()
        .map(|r| {
            let qty: i64 = r.get("quantity");
            let price: f64 = r.get("sale_price");
            serde_json::json!({
                "id":          r.get::<i64, _>("id"),
                "description": r.get::<Option<String>, _>("description").unwrap_or_default(),
                "set_code":    r.get::<Option<String>, _>("set_code").unwrap_or_default(),
                "condition":   r.get::<Option<String>, _>("condition").unwrap_or_default(),
                "quantity":    qty,
                "sale_price":  price,
                "line_total":  qty as f64 * price,
                "currency":    r.get::<String, _>("currency"),
            })
        })
        .collect();

    let tmpl = state
        .env
        .get_template("sales_detail.html")
        .expect("sales_detail.html missing");
    let ctx = minijinja::context! { txn => txn, items => items };
    Html(tmpl.render(ctx).expect("template render failed"))
}

// ── Shipping label download ───────────────────────────────────────────────────

pub async fn sale_label(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Response {
    let row = sqlx::query(
        "SELECT buyer_name, buyer_address, buyer_city, buyer_state, buyer_zip,
                platform_order_id, invoice_id
         FROM transactions WHERE id = ?",
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
    let oid     = row
        .get::<Option<String>, _>("invoice_id")
        .or_else(|| row.get::<Option<String>, _>("platform_order_id"))
        .unwrap_or_else(|| format!("#{}", id));

    let csz = format!("{} {} {}", city, state_s, zip).trim().to_string();
    let content = format!("{}|{}|{}|{}\n", name, addr, csz, oid);
    let disposition = format!("attachment; filename=\"label_{}.txt\"", id);

    let mut resp = content.into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
    resp.headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition.parse().unwrap());
    resp
}

