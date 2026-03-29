use axum::{
    extract::{Multipart, Path, Query, State},
    response::{Html, IntoResponse},
    Form,
};
use minijinja::context;
use serde::Deserialize;
use std::path::PathBuf;
use crate::{errors::AppError, state::AppState, db::unique_individual_id};

#[derive(Debug, Deserialize)]
pub struct IndividualsQuery {
    pub q: Option<String>,
    pub status: Option<String>,
    pub page: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct NewIndividualQuery {
    pub lot_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateIndividualForm {
    pub lot_id: Option<i64>,
    pub scryfall_id: String,
    pub foil: Option<String>,
    pub condition: String,
    pub cost_basis_cents: Option<i64>,
    pub location_id: Option<i64>,
    pub notes: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StatusForm {
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct LocationForm {
    pub location_id: Option<i64>,
}

pub async fn get_individuals(
    State(state): State<AppState>,
    Query(params): Query<IndividualsQuery>,
) -> Result<impl IntoResponse, AppError> {
    let page = params.page.unwrap_or(1).max(1);
    let per_page: i64 = 50;
    let offset = (page - 1) * per_page;

    let mut conditions = Vec::new();
    if params.q.is_some() {
        conditions.push("sc.name LIKE ?".to_string());
    }
    if params.status.is_some() {
        conditions.push("ic.status = ?".to_string());
    }
    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let q_pattern = params.q.as_deref().map(|q| format!("%{q}%"));

    let count_sql = format!(
        "SELECT COUNT(*) FROM individual_cards ic
         JOIN scryfall_cards sc ON sc.scryfall_id = ic.scryfall_id
         {where_clause}"
    );
    let data_sql = format!(
        "SELECT ic.id, ic.scryfall_id, ic.status, ic.foil, ic.condition,
                ic.cost_basis_cents, ic.notes,
                sc.name, sc.set_code, sc.set_name, sc.image_uri,
                sl.name as location_name,
                ic.created_at
         FROM individual_cards ic
         JOIN scryfall_cards sc ON sc.scryfall_id = ic.scryfall_id
         LEFT JOIN storage_locations sl ON sl.id = ic.location_id
         {where_clause}
         ORDER BY ic.created_at DESC
         LIMIT ? OFFSET ?"
    );

    macro_rules! bind_filters {
        ($q:expr) => {{
            let mut q = $q;
            if let Some(p) = &q_pattern { q = q.bind(p); }
            if let Some(s) = &params.status { q = q.bind(s); }
            q
        }};
    }

    let total: i64 = {
        let row = bind_filters!(sqlx::query(&count_sql)).fetch_one(&state.db).await?;
        use sqlx::Row;
        row.get::<i64, _>(0)
    };

    let rows = {
        bind_filters!(sqlx::query(&data_sql))
            .bind(per_page).bind(offset).fetch_all(&state.db).await?
    };

    use sqlx::Row;
    let cards: Vec<serde_json::Value> = rows.iter().map(|row| {
        let foil: i64 = row.get("foil");
        serde_json::json!({
            "id": row.get::<String, _>("id"),
            "scryfall_id": row.get::<String, _>("scryfall_id"),
            "name": row.get::<String, _>("name"),
            "set_code": row.get::<Option<String>, _>("set_code"),
            "set_name": row.get::<Option<String>, _>("set_name"),
            "image_uri": row.get::<Option<String>, _>("image_uri"),
            "status": row.get::<String, _>("status"),
            "foil": foil == 1,
            "condition": row.get::<String, _>("condition"),
            "cost_basis_cents": row.get::<Option<i64>, _>("cost_basis_cents"),
            "notes": row.get::<Option<String>, _>("notes"),
            "location_name": row.get::<Option<String>, _>("location_name"),
            "created_at": row.get::<String, _>("created_at"),
        })
    }).collect();

    let total_pages = (total + per_page - 1) / per_page;

    let tmpl = state.tmpl.get_template("individuals.html")?;
    let html = tmpl.render(context! {
        cards => cards,
        page => page,
        total_pages => total_pages,
        total => total,
        params => serde_json::json!({
            "q": params.q,
            "status": params.status,
        }),
    })?;
    Ok(Html(html))
}

pub async fn get_new_individual(
    State(state): State<AppState>,
    Query(params): Query<NewIndividualQuery>,
) -> Result<impl IntoResponse, AppError> {
    let lot = if let Some(lot_id) = params.lot_id {
        let row = sqlx::query(
            "SELECT il.id, il.scryfall_id, il.foil, il.condition, il.quantity,
                    sc.name, sc.set_code, sc.set_name
             FROM inventory_lots il
             JOIN scryfall_cards sc ON sc.scryfall_id = il.scryfall_id
             WHERE il.id = ?"
        )
        .bind(lot_id)
        .fetch_optional(&state.db)
        .await?;

        row.map(|r| {
            use sqlx::Row;
            let foil: i64 = r.get("foil");
            serde_json::json!({
                "id": r.get::<i64, _>("id"),
                "scryfall_id": r.get::<String, _>("scryfall_id"),
                "name": r.get::<String, _>("name"),
                "set_code": r.get::<Option<String>, _>("set_code"),
                "foil": foil == 1,
                "condition": r.get::<String, _>("condition"),
                "quantity": r.get::<i64, _>("quantity"),
            })
        })
    } else {
        None
    };

    let locations = get_locations_list(&state).await?;

    let tmpl = state.tmpl.get_template("individual_new.html")?;
    let html = tmpl.render(context! {
        lot => lot,
        locations => locations,
    })?;
    Ok(Html(html))
}

pub async fn post_individual(
    State(state): State<AppState>,
    Form(form): Form<CreateIndividualForm>,
) -> Result<impl IntoResponse, AppError> {
    let id = unique_individual_id(&state.db).await
        .map_err(|e| AppError::Internal(e))?;

    let foil = form.foil.as_deref().unwrap_or("") == "1";

    sqlx::query(
        "INSERT INTO individual_cards (id, lot_id, scryfall_id, foil, condition, cost_basis_cents, location_id, notes)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
    )
    .bind(&id)
    .bind(form.lot_id)
    .bind(&form.scryfall_id)
    .bind(foil as i64)
    .bind(&form.condition)
    .bind(form.cost_basis_cents)
    .bind(form.location_id)
    .bind(&form.notes)
    .execute(&state.db)
    .await?;

    Ok(axum::response::Redirect::to(&format!("/individuals/{id}")))
}

pub async fn get_individual_detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let row = sqlx::query(
        "SELECT ic.id, ic.scryfall_id, ic.lot_id, ic.status, ic.foil, ic.condition,
                ic.cost_basis_cents, ic.scan_front_path, ic.scan_back_path, ic.notes,
                ic.location_id, ic.created_at, ic.updated_at,
                sc.name, sc.set_code, sc.set_name, sc.collector_number,
                sc.image_uri, sc.mana_cost, sc.type_line, sc.rarity,
                sc.current_price_usd, sc.current_price_usd_foil,
                sl.name as location_name
         FROM individual_cards ic
         JOIN scryfall_cards sc ON sc.scryfall_id = ic.scryfall_id
         LEFT JOIN storage_locations sl ON sl.id = ic.location_id
         WHERE ic.id = ?"
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound(format!("Individual card '{id}' not found")))?;

    use sqlx::Row;
    let foil: i64 = row.get("foil");
    let card = serde_json::json!({
        "id": row.get::<String, _>("id"),
        "scryfall_id": row.get::<String, _>("scryfall_id"),
        "lot_id": row.get::<Option<i64>, _>("lot_id"),
        "status": row.get::<String, _>("status"),
        "foil": foil == 1,
        "condition": row.get::<String, _>("condition"),
        "cost_basis_cents": row.get::<Option<i64>, _>("cost_basis_cents"),
        "scan_front_path": row.get::<Option<String>, _>("scan_front_path"),
        "scan_back_path": row.get::<Option<String>, _>("scan_back_path"),
        "notes": row.get::<Option<String>, _>("notes"),
        "location_id": row.get::<Option<i64>, _>("location_id"),
        "location_name": row.get::<Option<String>, _>("location_name"),
        "created_at": row.get::<String, _>("created_at"),
        "updated_at": row.get::<String, _>("updated_at"),
        "name": row.get::<String, _>("name"),
        "set_code": row.get::<Option<String>, _>("set_code"),
        "set_name": row.get::<Option<String>, _>("set_name"),
        "collector_number": row.get::<Option<String>, _>("collector_number"),
        "image_uri": row.get::<Option<String>, _>("image_uri"),
        "mana_cost": row.get::<Option<String>, _>("mana_cost"),
        "type_line": row.get::<Option<String>, _>("type_line"),
        "rarity": row.get::<Option<String>, _>("rarity"),
        "price_usd": row.get::<Option<f64>, _>("current_price_usd"),
        "price_usd_foil": row.get::<Option<f64>, _>("current_price_usd_foil"),
    });

    let locations = get_locations_list(&state).await?;

    let tmpl = state.tmpl.get_template("individual_detail.html")?;
    let html = tmpl.render(context! {
        card => card,
        locations => locations,
    })?;
    Ok(Html(html))
}

pub async fn post_individual_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<StatusForm>,
) -> Result<impl IntoResponse, AppError> {
    sqlx::query(
        "UPDATE individual_cards SET status = ?, updated_at = datetime('now') WHERE id = ?"
    )
    .bind(&form.status)
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(axum::response::Redirect::to(&format!("/individuals/{id}")))
}

pub async fn post_individual_location(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<LocationForm>,
) -> Result<impl IntoResponse, AppError> {
    sqlx::query(
        "UPDATE individual_cards SET location_id = ?, updated_at = datetime('now') WHERE id = ?"
    )
    .bind(form.location_id)
    .bind(&id)
    .execute(&state.db)
    .await?;
    Ok(axum::response::Redirect::to(&format!("/individuals/{id}")))
}

pub async fn post_individual_scans(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let scan_dir = PathBuf::from(&state.scan_storage_path);
    tokio::fs::create_dir_all(&scan_dir).await?;

    let mut front_path: Option<String> = None;
    let mut back_path: Option<String> = None;

    while let Some(field) = multipart.next_field().await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        let field_name = field.name().unwrap_or("").to_string();
        let filename = field.file_name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{field_name}.bin"));
        let ext = std::path::Path::new(&filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg");

        let data = field.bytes().await
            .map_err(|e| AppError::BadRequest(format!("Failed to read scan: {e}")))?;

        if data.is_empty() { continue; }

        let stored_name = format!("{id}_{field_name}.{ext}");
        let stored_path = scan_dir.join(&stored_name);
        tokio::fs::write(&stored_path, &data).await?;
        let relative = format!("scans/{stored_name}");

        match field_name.as_str() {
            "front" => front_path = Some(relative),
            "back" => back_path = Some(relative),
            _ => {}
        }
    }

    if let Some(path) = front_path {
        sqlx::query(
            "UPDATE individual_cards SET scan_front_path = ?, updated_at = datetime('now') WHERE id = ?"
        )
        .bind(&path)
        .bind(&id)
        .execute(&state.db)
        .await?;
    }
    if let Some(path) = back_path {
        sqlx::query(
            "UPDATE individual_cards SET scan_back_path = ?, updated_at = datetime('now') WHERE id = ?"
        )
        .bind(&path)
        .bind(&id)
        .execute(&state.db)
        .await?;
    }

    Ok(axum::response::Redirect::to(&format!("/individuals/{id}")))
}

pub async fn get_individual_qr(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    // Verify card exists
    let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM individual_cards WHERE id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap_or(false);

    if !exists {
        return Err(AppError::NotFound(format!("Card '{id}' not found")));
    }

    let qr_svg = generate_qr_svg(&id)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("QR generation failed: {e}")))?;

    let tmpl = state.tmpl.get_template("qr_label.html")?;
    let html = tmpl.render(context! {
        card_id => id,
        qr_svg => qr_svg,
    })?;
    Ok(Html(html))
}

fn generate_qr_svg(data: &str) -> anyhow::Result<String> {
    use qrcode::{QrCode, EcLevel};
    use qrcode::render::svg;

    let code = QrCode::with_error_correction_level(data, EcLevel::M)
        .map_err(|e| anyhow::anyhow!("QR encode error: {e}"))?;

    let svg_string = code.render::<svg::Color>()
        .min_dimensions(80, 80)
        .max_dimensions(120, 120)
        .quiet_zone(false)
        .build();

    Ok(svg_string)
}

async fn get_locations_list(state: &AppState) -> Result<Vec<serde_json::Value>, AppError> {
    let rows = sqlx::query("SELECT id, name, type FROM storage_locations ORDER BY name")
        .fetch_all(&state.db)
        .await?;

    use sqlx::Row;
    Ok(rows.iter().map(|r| serde_json::json!({
        "id": r.get::<i64, _>("id"),
        "name": r.get::<String, _>("name"),
        "type": r.get::<String, _>("type"),
    })).collect())
}
