use axum::{
    extract::{Form, Path, State},
    response::{Html, Redirect},
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

pub async fn locations_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let rows = sqlx::query(
        r#"SELECT sl.id, "type" as location_type, sl.name, sl.description,
                  COUNT(il.id) as lot_count
           FROM storage_locations sl
           LEFT JOIN inventory_lots il ON il.location_id = sl.id
           GROUP BY sl.id ORDER BY sl.name ASC"#
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let locations: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
        "id": r.get::<i64, _>("id"),
        "location_type": r.get::<String, _>("location_type"),
        "name": r.get::<String, _>("name"),
        "description": r.get::<Option<String>, _>("description"),
        "lot_count": r.get::<i64, _>("lot_count"),
    })).collect();

    let tmpl = state.env.get_template("locations.html").expect("locations.html missing");
    let ctx = minijinja::context! { locations => locations };
    Html(tmpl.render(ctx).expect("template render failed"))
}

#[derive(Deserialize)]
pub struct NewLocation {
    pub name: String,
    pub location_type: String,
    pub description: Option<String>,
}

pub async fn create_location(
    State(state): State<Arc<AppState>>,
    Form(form): Form<NewLocation>,
) -> Redirect {
    let now = unix_now();
    let _ = sqlx::query(
        r#"INSERT INTO storage_locations ("type", name, description, created_at) VALUES (?, ?, ?, ?)"#
    )
    .bind(&form.location_type)
    .bind(&form.name)
    .bind(&form.description)
    .bind(now)
    .execute(&state.pool)
    .await;
    Redirect::to("/locations")
}

pub async fn delete_location(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> Redirect {
    let _ = sqlx::query("DELETE FROM storage_locations WHERE id = ?")
        .bind(id)
        .execute(&state.pool)
        .await;
    Redirect::to("/locations")
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
