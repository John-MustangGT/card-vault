use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse},
    Form,
};
use minijinja::context;
use serde::Deserialize;
use crate::{errors::AppError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct CreateLocationForm {
    pub name: String,
    #[serde(rename = "type")]
    pub location_type: String,
    pub description: Option<String>,
}

pub async fn get_locations(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let rows = sqlx::query(
        "SELECT sl.id, sl.name, sl.type, sl.description, sl.created_at,
                COUNT(DISTINCT il.id) as lot_count,
                COUNT(DISTINCT ic.id) as individual_count
         FROM storage_locations sl
         LEFT JOIN inventory_lots il ON il.location_id = sl.id
         LEFT JOIN individual_cards ic ON ic.location_id = sl.id
         GROUP BY sl.id
         ORDER BY sl.name"
    )
    .fetch_all(&state.db)
    .await?;

    use sqlx::Row;
    let locations: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
        "id": r.get::<i64, _>("id"),
        "name": r.get::<String, _>("name"),
        "type": r.get::<String, _>("type"),
        "description": r.get::<Option<String>, _>("description"),
        "created_at": r.get::<String, _>("created_at"),
        "lot_count": r.get::<i64, _>("lot_count"),
        "individual_count": r.get::<i64, _>("individual_count"),
    })).collect();

    let tmpl = state.tmpl.get_template("locations.html")?;
    let html = tmpl.render(context! {
        locations => locations,
    })?;
    Ok(Html(html))
}

pub async fn get_new_location(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let tmpl = state.tmpl.get_template("location_new.html")?;
    let html = tmpl.render(context! {})?;
    Ok(Html(html))
}

pub async fn post_location(
    State(state): State<AppState>,
    Form(form): Form<CreateLocationForm>,
) -> Result<impl IntoResponse, AppError> {
    if form.name.trim().is_empty() {
        return Err(AppError::BadRequest("Location name is required".into()));
    }
    let valid_types = ["binder", "box", "other"];
    if !valid_types.contains(&form.location_type.as_str()) {
        return Err(AppError::BadRequest("Invalid location type".into()));
    }

    sqlx::query(
        "INSERT INTO storage_locations (name, type, description) VALUES (?, ?, ?)"
    )
    .bind(form.name.trim())
    .bind(&form.location_type)
    .bind(form.description.as_deref().filter(|s| !s.is_empty()))
    .execute(&state.db)
    .await?;

    Ok(axum::response::Redirect::to("/locations"))
}

pub async fn get_location_detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query(
        "SELECT id, name, type, description, created_at FROM storage_locations WHERE id = ?"
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("Location {id} not found")))?;

    use sqlx::Row;
    let location = serde_json::json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "type": row.get::<String, _>("type"),
        "description": row.get::<Option<String>, _>("description"),
        "created_at": row.get::<String, _>("created_at"),
    });

    // Lots stored here
    let lot_rows = sqlx::query(
        "SELECT il.id, sc.name, sc.set_code, il.foil, il.condition, il.quantity
         FROM inventory_lots il
         JOIN scryfall_cards sc ON sc.scryfall_id = il.scryfall_id
         WHERE il.location_id = ?
         ORDER BY sc.name"
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let lots: Vec<serde_json::Value> = lot_rows.iter().map(|r| {
        let foil: i64 = r.get("foil");
        serde_json::json!({
            "id": r.get::<i64, _>("id"),
            "name": r.get::<String, _>("name"),
            "set_code": r.get::<Option<String>, _>("set_code"),
            "foil": foil == 1,
            "condition": r.get::<String, _>("condition"),
            "quantity": r.get::<i64, _>("quantity"),
        })
    }).collect();

    // Individual cards stored here
    let ind_rows = sqlx::query(
        "SELECT ic.id, sc.name, sc.set_code, ic.foil, ic.condition, ic.status
         FROM individual_cards ic
         JOIN scryfall_cards sc ON sc.scryfall_id = ic.scryfall_id
         WHERE ic.location_id = ?
         ORDER BY sc.name"
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;

    let individuals: Vec<serde_json::Value> = ind_rows.iter().map(|r| {
        let foil: i64 = r.get("foil");
        serde_json::json!({
            "id": r.get::<String, _>("id"),
            "name": r.get::<String, _>("name"),
            "set_code": r.get::<Option<String>, _>("set_code"),
            "foil": foil == 1,
            "condition": r.get::<String, _>("condition"),
            "status": r.get::<String, _>("status"),
        })
    }).collect();

    let is_empty = lots.is_empty() && individuals.is_empty();

    let tmpl = state.tmpl.get_template("location_detail.html")?;
    let html = tmpl.render(context! {
        location => location,
        lots => lots,
        individuals => individuals,
        is_empty => is_empty,
    })?;
    Ok(Html(html))
}

pub async fn post_location_update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<CreateLocationForm>,
) -> Result<impl IntoResponse, AppError> {
    if form.name.trim().is_empty() {
        return Err(AppError::BadRequest("Location name is required".into()));
    }
    sqlx::query(
        "UPDATE storage_locations SET name = ?, type = ?, description = ? WHERE id = ?"
    )
    .bind(form.name.trim())
    .bind(&form.location_type)
    .bind(form.description.as_deref().filter(|s| !s.is_empty()))
    .bind(id)
    .execute(&state.db)
    .await?;

    Ok(axum::response::Redirect::to(&format!("/locations/{id}")))
}

pub async fn post_location_delete(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    // Check if empty
    let lot_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM inventory_lots WHERE location_id = ?"
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    let ind_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM individual_cards WHERE location_id = ?"
    )
    .bind(id)
    .fetch_one(&state.db)
    .await?;

    if lot_count > 0 || ind_count > 0 {
        return Err(AppError::BadRequest(
            "Cannot delete location with cards stored in it".into()
        ));
    }

    sqlx::query("DELETE FROM storage_locations WHERE id = ?")
        .bind(id)
        .execute(&state.db)
        .await?;

    Ok(axum::response::Redirect::to("/locations"))
}
