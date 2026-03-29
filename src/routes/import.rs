use axum::{
    extract::{Multipart, State},
    response::{Html, IntoResponse},
};
use minijinja::context;
use crate::{errors::AppError, state::AppState};

pub async fn get_import(State(state): State<AppState>) -> Result<impl IntoResponse, AppError> {
    let tmpl = state.tmpl.get_template("import.html")?;
    let html = tmpl.render(context! {})?;
    Ok(Html(html))
}

pub async fn post_import(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, AppError> {
    let mut file_data: Option<Vec<u8>> = None;
    let mut conflict_strategy = "accumulate".to_string();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let data = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read file: {e}")))?;
                file_data = Some(data.to_vec());
            }
            "conflict_strategy" => {
                conflict_strategy = field
                    .text()
                    .await
                    .unwrap_or_else(|_| "accumulate".to_string());
            }
            _ => {}
        }
    }

    let data = match file_data {
        Some(d) if !d.is_empty() => d,
        _ => {
            let tmpl = state.tmpl.get_template("import.html")?;
            let html = tmpl.render(context! {
                error => "No file uploaded"
            })?;
            return Ok(Html(html));
        }
    };

    match crate::db::import::import_csv(&state.db, &data, &conflict_strategy).await {
        Ok(result) => {
            let tmpl = state.tmpl.get_template("import.html")?;
            let html = tmpl.render(context! {
                result => serde_json::to_value(&result).unwrap_or_default(),
            })?;
            Ok(Html(html))
        }
        Err(e) => {
            let tmpl = state.tmpl.get_template("import.html")?;
            let html = tmpl.render(context! {
                error => format!("Import failed: {e}"),
            })?;
            Ok(Html(html))
        }
    }
}
