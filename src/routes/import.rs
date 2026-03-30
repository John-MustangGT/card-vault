use axum::{
    extract::{Multipart, State},
    response::Html,
};
use std::sync::Arc;
use tracing::info;

use crate::db::import::{import_cardsphere_csv, import_manabox_csv};
use crate::AppState;

pub async fn import_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let tmpl = state.env.get_template("import.html").expect("import.html missing");
    Html(tmpl.render(minijinja::context!()).expect("template render failed"))
}

pub async fn handle_import(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Html<String> {
    let mut csv_data: Option<String> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("csv_file") {
            match field.bytes().await {
                Ok(bytes) => csv_data = Some(String::from_utf8_lossy(&bytes).into_owned()),
                Err(e) => return Html(error_html(&format!("Failed to read upload: {e}"))),
            }
        }
    }

    let csv = match csv_data {
        Some(d) if !d.trim().is_empty() => d,
        _ => return Html(error_html("No CSV file provided.")),
    };

    // Auto-detect format from header row
    let first_line = csv.lines().next().unwrap_or("");
    let is_cardsphere = first_line.contains("Tradelist Count") || first_line.contains("Cardsphere ID");

    let import_result = if is_cardsphere {
        info!("Detected CardSphere format");
        import_cardsphere_csv(&state.pool, &csv).await
    } else {
        info!("Detected ManaBox format");
        import_manabox_csv(&state.pool, &csv).await
    };

    match import_result {
        Ok(result) => Html(success_html(result)),
        Err(e) => Html(error_html(&e.to_string())),
    }
}

fn success_html(r: crate::db::import::ImportResult) -> String {
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
  <p class="result-message">Import complete — {rows} rows processed, {lots} lots upserted.</p>
  {error_rows}
  <div class="result-actions">
    <a href="/inventory" class="btn">View Inventory</a>
    <a href="/import" class="btn btn-secondary">Import Another</a>
  </div>
</div>"#,
        rows = r.rows_processed,
        lots = r.lots_upserted,
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
