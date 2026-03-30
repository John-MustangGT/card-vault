use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;

use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct IndividualsQuery {
    pub status: Option<String>,
    pub q: Option<String>,
}

pub async fn individuals_page(
    State(state): State<Arc<AppState>>,
    Query(params): Query<IndividualsQuery>,
) -> Html<String> {
    let rows = sqlx::query(
        r#"SELECT ic.id, ic.scryfall_id, ic.foil, ic.condition, ic.status,
                  ic.acquisition_cost, ic.notes,
                  sc.name, sc.set_code, sc.collector_number,
                  sl.name as location_name
           FROM individual_cards ic
           JOIN scryfall_cards sc ON sc.scryfall_id = ic.scryfall_id
           LEFT JOIN storage_locations sl ON sl.id = ic.location_id
           ORDER BY ic.created_at DESC"#
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let mut individuals: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
        "id": r.get::<String, _>("id"),
        "scryfall_id": r.get::<String, _>("scryfall_id"),
        "name": r.get::<String, _>("name"),
        "set_code": r.get::<String, _>("set_code"),
        "collector_number": r.get::<String, _>("collector_number"),
        "foil": r.get::<String, _>("foil"),
        "condition": r.get::<String, _>("condition"),
        "status": r.get::<String, _>("status"),
        "acquisition_cost": r.get::<Option<f64>, _>("acquisition_cost"),
        "notes": r.get::<Option<String>, _>("notes").unwrap_or_default(),
        "location_name": r.get::<Option<String>, _>("location_name").unwrap_or_default(),
    })).collect();

    // Filter in Rust
    if let Some(q) = &params.q {
        if !q.is_empty() {
            let q_lower = q.to_lowercase();
            individuals.retain(|r| {
                r["name"].as_str().unwrap_or("").to_lowercase().contains(&q_lower)
                || r["id"].as_str().unwrap_or("").to_lowercase().contains(&q_lower)
            });
        }
    }
    if let Some(status) = &params.status {
        if !status.is_empty() {
            individuals.retain(|r| r["status"].as_str().unwrap_or("") == status.as_str());
        }
    }

    let tmpl = state.env.get_template("individuals.html").expect("individuals.html missing");
    let ctx = minijinja::context! {
        individuals => individuals,
        filter_q => params.q.unwrap_or_default(),
        filter_status => params.status.unwrap_or_default(),
    };
    Html(tmpl.render(ctx).expect("template render failed"))
}
