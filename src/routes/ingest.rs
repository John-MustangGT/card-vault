use axum::{
    extract::{Multipart, State},
    Json,
};
use serde_json::json;
use sqlx::Row;
use std::sync::Arc;
use tracing::{info, warn};

use crate::AppState;

/// POST /api/ingest
///
/// Accepts a multipart body from the scanner recognition script:
///   - scryfall_id   (text)
///   - condition     (text, optional — defaults to "near_mint")
///   - foil          (text, optional — defaults to "normal")
///   - front         (file, optional — front scan image)
///   - back          (file, optional — back scan image)
///
/// Creates an individual_cards entry, pulls the next UID from the pool,
/// saves scans to disk, and returns JSON: { ok, card_id, name, set_code }
pub async fn ingest(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Json<serde_json::Value> {
    // --- Collect fields ---
    let mut scryfall_id = String::new();
    let mut condition = String::from("near_mint");
    let mut foil = String::from("normal");
    let mut front_bytes: Option<(String, Vec<u8>)> = None; // (content_type, bytes)
    let mut back_bytes: Option<(String, Vec<u8>)> = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "scryfall_id" => {
                scryfall_id = field.text().await.unwrap_or_default().trim().to_string();
            }
            "condition" => {
                let v = field.text().await.unwrap_or_default();
                let v = v.trim();
                if !v.is_empty() {
                    condition = v.to_string();
                }
            }
            "foil" => {
                let v = field.text().await.unwrap_or_default();
                let v = v.trim();
                if !v.is_empty() {
                    foil = v.to_string();
                }
            }
            "front" => {
                let ct = field
                    .content_type()
                    .unwrap_or("image/jpeg")
                    .to_string();
                let bytes = field.bytes().await.unwrap_or_default();
                if !bytes.is_empty() {
                    front_bytes = Some((ct, bytes.to_vec()));
                }
            }
            "back" => {
                let ct = field
                    .content_type()
                    .unwrap_or("image/jpeg")
                    .to_string();
                let bytes = field.bytes().await.unwrap_or_default();
                if !bytes.is_empty() {
                    back_bytes = Some((ct, bytes.to_vec()));
                }
            }
            _ => {}
        }
    }

    if scryfall_id.is_empty() {
        return Json(json!({ "ok": false, "error": "scryfall_id is required" }));
    }

    // --- Validate card exists ---
    let card = sqlx::query(
        "SELECT scryfall_id, name, set_code FROM scryfall_cards WHERE scryfall_id = ?",
    )
    .bind(&scryfall_id)
    .fetch_optional(&state.pool)
    .await;

    let card = match card {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Json(json!({
                "ok": false,
                "error": format!("scryfall_id not found: {}", scryfall_id)
            }))
        }
        Err(e) => {
            return Json(json!({ "ok": false, "error": format!("db error: {}", e) }))
        }
    };

    let card_name: String = card.get("name");
    let set_code: String = card.get("set_code");

    // --- Pull next UID from pool ---
    let uid_row = sqlx::query(
        "SELECT id, uid FROM uid_pool WHERE used = 0 ORDER BY id LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await;

    let (pool_row_id, card_id) = match uid_row {
        Ok(Some(r)) => (r.get::<i64, _>("id"), r.get::<String, _>("uid")),
        Ok(None) => {
            return Json(json!({
                "ok": false,
                "error": "UID pool is empty — generate more labels first"
            }))
        }
        Err(e) => {
            return Json(json!({ "ok": false, "error": format!("uid pool error: {}", e) }))
        }
    };

    let now = chrono::Utc::now().timestamp();

    // --- Insert individual_cards ---
    let insert = sqlx::query(
        "INSERT INTO individual_cards
             (id, scryfall_id, foil, condition, status, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'in_stock', ?, ?)",
    )
    .bind(&card_id)
    .bind(&scryfall_id)
    .bind(&foil)
    .bind(&condition)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await;

    if let Err(e) = insert {
        return Json(json!({ "ok": false, "error": format!("insert failed: {}", e) }));
    }

    // --- Mark UID as used ---
    let _ = sqlx::query(
        "UPDATE uid_pool SET used = 1, card_id = ?, used_at = ? WHERE id = ?",
    )
    .bind(&card_id)
    .bind(now)
    .bind(pool_row_id)
    .execute(&state.pool)
    .await;

    // --- Save scan images ---
    let scan_dir = std::path::PathBuf::from(&state.config.scan_storage_path).join(&card_id);
    let mut front_path: Option<String> = None;
    let mut back_path: Option<String> = None;

    if let Some((ct, bytes)) = front_bytes {
        let ext = img_ext(&ct);
        if let Err(e) = std::fs::create_dir_all(&scan_dir) {
            warn!("Failed to create scan dir {:?}: {}", scan_dir, e);
        } else {
            let fname = format!("front.{}", ext);
            let fpath = scan_dir.join(&fname);
            if std::fs::write(&fpath, &bytes).is_ok() {
                front_path = Some(format!("{}/{}", &card_id, &fname));
            }
        }
    }

    if let Some((ct, bytes)) = back_bytes {
        let ext = img_ext(&ct);
        if let Err(e) = std::fs::create_dir_all(&scan_dir) {
            warn!("Failed to create scan dir {:?}: {}", scan_dir, e);
        } else {
            let fname = format!("back.{}", ext);
            let fpath = scan_dir.join(&fname);
            if std::fs::write(&fpath, &bytes).is_ok() {
                back_path = Some(format!("{}/{}", &card_id, &fname));
            }
        }
    }

    // Update scan paths if we saved any
    if front_path.is_some() || back_path.is_some() {
        let _ = sqlx::query(
            "UPDATE individual_cards
             SET scan_front_path = COALESCE(?, scan_front_path),
                 scan_back_path  = COALESCE(?, scan_back_path),
                 scan_updated_at = ?,
                 updated_at = ?
             WHERE id = ?",
        )
        .bind(&front_path)
        .bind(&back_path)
        .bind(now)
        .bind(now)
        .bind(&card_id)
        .execute(&state.pool)
        .await;
    }

    info!(
        "Ingested via scanner: {} ({}) — card_id={} set={}",
        card_name, scryfall_id, card_id, set_code
    );

    Json(json!({
        "ok": true,
        "card_id": card_id,
        "name": card_name,
        "set_code": set_code,
        "scryfall_id": scryfall_id,
        "condition": condition,
        "foil": foil,
    }))
}

fn img_ext(content_type: &str) -> String {
    match content_type {
        ct if ct.contains("png") => "png".into(),
        ct if ct.contains("webp") => "webp".into(),
        ct if ct.contains("gif") => "gif".into(),
        ct if ct.contains("tiff") => "tiff".into(),
        _ => "jpg".into(),
    }
}
