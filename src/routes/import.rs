use axum::{
    extract::{Multipart, State},
    response::Html,
};
use sqlx::Row;
use std::sync::Arc;
use tracing::info;

use crate::db::import::{import_cardsphere_csv, import_manabox_csv};
use crate::AppState;

pub async fn import_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let rows = sqlx::query(
        r#"SELECT id, "type" AS location_type, name FROM storage_locations
           WHERE "type" != 'chaos' ORDER BY name ASC"#,
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();

    let locations: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.get::<i64, _>("id"),
                "location_type": r.get::<String, _>("location_type"),
                "name": r.get::<String, _>("name"),
            })
        })
        .collect();

    let tmpl = state.env.get_template("import.html").expect("import.html missing");
    Html(tmpl.render(minijinja::context! { locations => locations }).expect("template render failed"))
}

pub async fn handle_import(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Html<String> {
    let mut csv_data: Option<String> = None;
    let mut filename = String::new();
    let mut dest = String::from("chaos");
    let mut location_id: Option<i64> = None;
    let mut new_name = String::new();
    let mut new_type = String::from("binder");

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "csv_file" => {
                filename = field.file_name().unwrap_or("").to_string();
                match field.bytes().await {
                    Ok(bytes) => csv_data = Some(String::from_utf8_lossy(&bytes).into_owned()),
                    Err(e) => return Html(error_html(&format!("Failed to read upload: {e}"))),
                }
            }
            "dest" => dest = field.text().await.unwrap_or_default(),
            "location_id" => {
                location_id = field.text().await.unwrap_or_default().trim().parse::<i64>().ok();
            }
            "new_location_name" => new_name = field.text().await.unwrap_or_default().trim().to_string(),
            "new_location_type" => new_type = field.text().await.unwrap_or_default(),
            _ => {}
        }
    }

    let csv = match csv_data {
        Some(d) if !d.trim().is_empty() => d,
        _ => return Html(error_html("No CSV file provided.")),
    };

    // Resolve the destination location. `created` marks a location made for this
    // import, so it can be removed again if the import adds nothing.
    let (location, created) = match dest.as_str() {
        "existing" => {
            let row = match location_id {
                Some(id) => sqlx::query("SELECT id, name FROM storage_locations WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&state.pool)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            match row {
                Some(r) => ((r.get::<i64, _>("id"), r.get::<String, _>("name")), false),
                None => return Html(error_html("Choose an existing location to import into.")),
            }
        }
        "new" => {
            if new_name.is_empty() {
                return Html(error_html("Enter a name for the new location."));
            }
            let new_type = if new_type == "box" { "box" } else { "binder" };
            match find_or_create_location(&state, &new_name, new_type, None).await {
                Ok(v) => v,
                Err(e) => return Html(error_html(&format!("Could not create location: {e}"))),
            }
        }
        _ => {
            let description = if filename.is_empty() {
                "Chaos warehouse import".to_string()
            } else {
                format!("Chaos warehouse import of {filename}")
            };
            match create_chaos_location(&state, &description).await {
                Ok(v) => (v, true),
                Err(e) => return Html(error_html(&format!("Could not create location: {e}"))),
            }
        }
    };
    let (loc_id, loc_name) = location;

    // Auto-detect format from header row
    let first_line = csv.lines().next().unwrap_or("");
    let is_cardsphere = first_line.contains("Tradelist Count") || first_line.contains("Cardsphere ID");

    let import_result = if is_cardsphere {
        info!("Detected CardSphere format");
        import_cardsphere_csv(&state.pool, &csv, Some(loc_id)).await
    } else {
        info!("Detected ManaBox format");
        import_manabox_csv(&state.pool, &csv, Some(loc_id)).await
    };

    let lots_added = import_result.as_ref().map(|r| r.lots_upserted).unwrap_or(0);
    if created && lots_added == 0 {
        let _ = sqlx::query("DELETE FROM storage_locations WHERE id = ?")
            .bind(loc_id)
            .execute(&state.pool)
            .await;
    }

    match import_result {
        Ok(result) => {
            crate::db::sets::spawn_ensure_sets(state.pool.clone());
            Html(success_html(result, &loc_name))
        }
        Err(e) => Html(error_html(&e.to_string())),
    }
}

async fn find_or_create_location(
    state: &AppState,
    name: &str,
    kind: &str,
    description: Option<&str>,
) -> Result<((i64, String), bool), sqlx::Error> {
    if let Some(r) = sqlx::query("SELECT id, name FROM storage_locations WHERE LOWER(name) = LOWER(?)")
        .bind(name)
        .fetch_optional(&state.pool)
        .await?
    {
        return Ok(((r.get("id"), r.get("name")), false));
    }
    let id = sqlx::query(
        r#"INSERT INTO storage_locations ("type", name, description, created_at) VALUES (?, ?, ?, ?)"#,
    )
    .bind(kind)
    .bind(name)
    .bind(description)
    .bind(unix_now())
    .execute(&state.pool)
    .await?
    .last_insert_rowid();
    Ok(((id, name.to_string()), true))
}

/// Create a location named with a fresh random id, e.g. `CHAOS-7KQ2M9XA`.
async fn create_chaos_location(state: &AppState, description: &str) -> Result<(i64, String), sqlx::Error> {
    // Unambiguous characters only (no 0/O, 1/I) since these get read off screens and labels.
    const ALPHABET: [char; 32] = [
        '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'G', 'H',
        'J', 'K', 'L', 'M', 'N', 'P', 'Q', 'R', 'S', 'T', 'U', 'V', 'W', 'X', 'Y', 'Z',
    ];
    loop {
        let name = format!("CHAOS-{}", nanoid::nanoid!(8, &ALPHABET));
        let taken = sqlx::query("SELECT 1 FROM storage_locations WHERE name = ?")
            .bind(&name)
            .fetch_optional(&state.pool)
            .await?
            .is_some();
        if !taken {
            return Ok(find_or_create_location(state, &name, "chaos", Some(description)).await?.0);
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn success_html(r: crate::db::import::ImportResult, location: &str) -> String {
    let error_rows = if r.errors.is_empty() {
        String::new()
    } else {
        let items: String = r
            .errors
            .iter()
            .map(|e| format!("<li>{}</li>", html_escape(e)))
            .collect();
        format!(
            r#"<div class="error-list"><h4>Warnings / skipped rows</h4><ul>{items}</ul></div>"#
        )
    };

    format!(
        r#"<div class="import-result success" style="margin-top:1.5rem">
  <p class="result-message">Import complete — {rows} rows processed, {lots} lots upserted into <strong>{location}</strong>.</p>
  {error_rows}
  <div class="result-actions">
    <a href="/inventory" class="btn">View Inventory</a>
    <a href="/import" class="btn btn-secondary">Import Another</a>
  </div>
</div>"#,
        rows = r.rows_processed,
        lots = r.lots_upserted,
        location = html_escape(location),
    )
}

fn error_html(msg: &str) -> String {
    format!(
        r#"<div class="import-result error" style="margin-top:1.5rem">
  <p class="result-message">Error: {}</p>
  <div class="result-actions">
    <a href="/import" class="btn btn-secondary">Try Again</a>
  </div>
</div>"#,
        html_escape(msg)
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
