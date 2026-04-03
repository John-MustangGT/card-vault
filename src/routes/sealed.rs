use axum::{
    extract::{Path, State},
    response::{Html, Redirect},
    Json,
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

pub async fn sealed_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let rows = sqlx::query(
        r#"SELECT sp.id, sp.product_type, sp.name, sp.set_code, sp.set_name,
                  sp.language, sp.quantity, sp.acquisition_cost, sp.notes,
                  sl.name as location_name
           FROM sealed_products sp
           LEFT JOIN storage_locations sl ON sl.id = sp.location_id
           ORDER BY sp.set_code, sp.product_type, sp.name"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let products: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id":           r.get::<i64, _>("id"),
                "product_type": r.get::<String, _>("product_type"),
                "name":         r.get::<String, _>("name"),
                "set_code":     r.get::<String, _>("set_code"),
                "set_name":     r.get::<String, _>("set_name"),
                "language":     r.get::<String, _>("language"),
                "quantity":     r.get::<i64, _>("quantity"),
                "acquisition_cost": r.get::<Option<f64>, _>("acquisition_cost"),
                "notes":        r.get::<Option<String>, _>("notes").unwrap_or_default(),
                "location_name": r.get::<Option<String>, _>("location_name").unwrap_or_default(),
            })
        })
        .collect();

    let locations = sqlx::query(
        "SELECT id, name, \"type\" as location_type FROM storage_locations ORDER BY name",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let locations: Vec<serde_json::Value> = locations
        .iter()
        .map(|r| {
            serde_json::json!({
                "id":   r.get::<i64, _>("id"),
                "name": r.get::<String, _>("name"),
            })
        })
        .collect();

    let tmpl = state.env.get_template("sealed.html").expect("sealed.html missing");
    let ctx = minijinja::context! { products => products, locations => locations };
    Html(tmpl.render(ctx).expect("template render failed"))
}

#[derive(Deserialize)]
pub struct SealedForm {
    pub product_type:        String,
    pub name:                String,
    pub set_code:            Option<String>,
    pub set_name:            Option<String>,
    pub language:            Option<String>,
    pub quantity:            i64,
    pub acquisition_cost:    Option<f64>,
    pub notes:               Option<String>,
    pub location_id:         Option<i64>,
}

pub async fn create_sealed(
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<SealedForm>,
) -> Redirect {
    let now = unix_now();
    let _ = sqlx::query(
        "INSERT INTO sealed_products
         (product_type, name, set_code, set_name, language, quantity,
          acquisition_cost, notes, location_id, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&form.product_type)
    .bind(&form.name)
    .bind(form.set_code.as_deref().unwrap_or(""))
    .bind(form.set_name.as_deref().unwrap_or(""))
    .bind(form.language.as_deref().unwrap_or("en"))
    .bind(form.quantity)
    .bind(form.acquisition_cost)
    .bind(form.notes.as_deref().filter(|s| !s.is_empty()))
    .bind(form.location_id)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await;

    Redirect::to("/sealed")
}

#[derive(Deserialize)]
pub struct AdjustQtyForm {
    pub delta: i64,  // positive = add, negative = remove
}

pub async fn adjust_qty(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(form): Json<AdjustQtyForm>,
) -> Json<serde_json::Value> {
    let now = unix_now();
    let _ = sqlx::query(
        "UPDATE sealed_products SET quantity = MAX(0, quantity + ?), updated_at = ? WHERE id = ?",
    )
    .bind(form.delta)
    .bind(now)
    .bind(id)
    .execute(&state.pool)
    .await;

    let qty: i64 = sqlx::query_scalar("SELECT quantity FROM sealed_products WHERE id = ?")
        .bind(id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(0);

    Json(serde_json::json!({ "ok": true, "quantity": qty }))
}

pub async fn delete_sealed(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Json<serde_json::Value> {
    let _ = sqlx::query("DELETE FROM sealed_products WHERE id = ?")
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
