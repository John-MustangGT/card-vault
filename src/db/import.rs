use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Raw ManaBox CSV row shape
#[derive(Debug, Deserialize)]
pub struct ManaboxRow {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Set code")]
    pub set_code: String,
    #[serde(rename = "Set name")]
    pub set_name: String,
    #[serde(rename = "Collector number")]
    pub collector_number: String,
    #[serde(rename = "Foil")]
    pub foil: String,
    #[serde(rename = "Rarity")]
    pub rarity: String,
    #[serde(rename = "Quantity")]
    pub quantity: i64,
    #[serde(rename = "ManaBox ID")]
    pub manabox_id: Option<i64>,
    #[serde(rename = "Scryfall ID")]
    pub scryfall_id: String,
    #[serde(rename = "Purchase price")]
    pub purchase_price: Option<f64>,
    #[serde(rename = "Misprint")]
    pub misprint: Option<String>,
    #[serde(rename = "Altered")]
    pub altered: Option<String>,
    #[serde(rename = "Condition")]
    pub condition: String,
    #[serde(rename = "Language")]
    pub language: String,
    #[serde(rename = "Purchase price currency")]
    pub purchase_price_currency: Option<String>,
}

/// CardSphere CSV export row shape
#[derive(Debug, Deserialize)]
pub struct CardSphereRow {
    #[serde(rename = "Count")]
    pub count: i64,
    #[serde(rename = "Tradelist Count")]
    pub tradelist_count: i64,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Edition")]
    pub edition: String,
    #[serde(rename = "Condition")]
    pub condition: String,   // "NM", "LP", "MP", "HP", "DMG"
    #[serde(rename = "Language")]
    pub language: String,    // "EN", "JP", etc.
    #[serde(rename = "Foil")]
    pub foil: String,        // "N" | "Y"
    #[serde(rename = "Tags")]
    pub tags: Option<String>,
    #[serde(rename = "Scryfall ID")]
    pub scryfall_id: Option<String>,
    #[serde(rename = "Cardsphere ID")]
    pub cardsphere_id: Option<i64>,
    #[serde(rename = "Last Modified")]
    pub last_modified: Option<String>,
}

fn normalize_condition(cs: &str) -> &'static str {
    match cs.to_uppercase().as_str() {
        "NM" | "NEAR_MINT"          => "near_mint",
        "LP" | "LIGHTLY_PLAYED"     => "lightly_played",
        "MP" | "MODERATELY_PLAYED"  => "moderately_played",
        "HP" | "HEAVILY_PLAYED"     => "heavily_played",
        "DMG" | "DAMAGED"           => "damaged",
        _                           => "near_mint",
    }
}

fn normalize_foil(f: &str) -> &'static str {
    match f.to_uppercase().as_str() {
        "Y" | "FOIL"   => "foil",
        _              => "normal",
    }
}

fn normalize_language(lang: &str) -> String {
    lang.to_lowercase()
}

fn tags_to_json(tags: &Option<String>) -> Option<String> {
    let t = tags.as_deref().unwrap_or("").trim();
    if t.is_empty() {
        None
    } else {
        let parts: Vec<&str> = t.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() { None } else { Some(serde_json::to_string(&parts).unwrap_or_default()) }
    }
}

#[derive(Debug, Default)]
pub struct ImportResult {
    pub rows_processed: usize,
    pub cards_upserted: usize,
    pub lots_upserted: usize,
    pub errors: Vec<String>,
}

pub async fn import_cardsphere_csv(pool: &SqlitePool, csv_data: &str) -> Result<ImportResult> {
    let mut result = ImportResult::default();
    let now = unix_now();

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(csv_data.as_bytes());

    for (i, record) in rdr.deserialize::<CardSphereRow>().enumerate() {
        result.rows_processed += 1;

        let row = match record {
            Ok(r) => r,
            Err(e) => {
                result.errors.push(format!("Row {}: {}", i + 2, e));
                continue;
            }
        };

        // Skip rows without a Scryfall ID (e.g. sealed booster packs)
        let scryfall_id = match &row.scryfall_id {
            Some(id) if !id.is_empty() => id.clone(),
            _ => {
                info!("Skipping '{}' — no Scryfall ID", row.name);
                result.errors.push(format!(
                    "Row {}: skipped '{}' ({}): no Scryfall ID",
                    i + 2, row.name, row.edition
                ));
                continue;
            }
        };

        let condition  = normalize_condition(&row.condition);
        let foil       = normalize_foil(&row.foil);
        let language   = normalize_language(&row.language);
        let tags_json  = tags_to_json(&row.tags);

        // Upsert scryfall_cards — only name + edition name from CardSphere,
        // no set_code or collector_number. Use empty strings as placeholders;
        // a future Scryfall API hydration pass will fill them in.
        let upsert_card = sqlx::query(
            r#"
            INSERT INTO scryfall_cards
                (scryfall_id, name, set_code, set_name, collector_number, rarity, language, cached_at)
            VALUES (?, ?, '', ?, '', '', ?, ?)
            ON CONFLICT(scryfall_id) DO UPDATE SET
                name      = CASE WHEN excluded.name != '' THEN excluded.name ELSE name END,
                set_name  = CASE WHEN excluded.set_name != '' THEN excluded.set_name ELSE set_name END,
                cached_at = excluded.cached_at
            "#,
        )
        .bind(&scryfall_id)
        .bind(&row.name)
        .bind(&row.edition)
        .bind(&language)
        .bind(now)
        .execute(pool)
        .await;

        match upsert_card {
            Ok(_) => result.cards_upserted += 1,
            Err(e) => {
                warn!("Failed to upsert card {}: {}", scryfall_id, e);
                result.errors.push(format!(
                    "Card upsert failed for '{}' ({}): {}", row.name, scryfall_id, e
                ));
                continue;
            }
        }

        let upsert_lot = sqlx::query(
            r#"
            INSERT INTO inventory_lots
                (scryfall_id, foil, condition, quantity, acquisition_currency,
                 tags, created_at, updated_at)
            VALUES (?, ?, ?, ?, 'USD', ?, ?, ?)
            ON CONFLICT(scryfall_id, foil, condition) DO UPDATE SET
                quantity   = quantity + excluded.quantity,
                tags       = COALESCE(excluded.tags, tags),
                updated_at = excluded.updated_at
            "#,
        )
        .bind(&scryfall_id)
        .bind(foil)
        .bind(condition)
        .bind(row.count)
        .bind(&tags_json)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await;

        match upsert_lot {
            Ok(_) => {
                result.lots_upserted += 1;
                info!("Upserted lot: {} x{}", row.name, row.count);
            }
            Err(e) => {
                result.errors.push(format!("Lot upsert failed for '{}': {}", row.name, e));
            }
        }
    }

    Ok(result)
}

pub async fn import_manabox_csv(pool: &SqlitePool, csv_data: &str) -> Result<ImportResult> {
    let mut result = ImportResult::default();
    let now = unix_now();

    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(csv_data.as_bytes());

    for (i, record) in rdr.deserialize::<ManaboxRow>().enumerate() {
        result.rows_processed += 1;

        let row = match record {
            Ok(r) => r,
            Err(e) => {
                result.errors.push(format!("Row {}: {}", i + 2, e));
                continue;
            }
        };

        // Upsert scryfall_cards cache (minimal data from CSV; image_uri hydrated later)
        let upsert_card = sqlx::query(
            r#"
            INSERT INTO scryfall_cards
                (scryfall_id, name, set_code, set_name, collector_number, rarity, language, cached_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(scryfall_id) DO UPDATE SET
                name             = excluded.name,
                set_code         = excluded.set_code,
                set_name         = excluded.set_name,
                collector_number = excluded.collector_number,
                rarity           = excluded.rarity,
                language         = excluded.language,
                cached_at        = excluded.cached_at
            "#,
        )
        .bind(&row.scryfall_id)
        .bind(&row.name)
        .bind(&row.set_code)
        .bind(&row.set_name)
        .bind(&row.collector_number)
        .bind(&row.rarity)
        .bind(&row.language)
        .bind(now)
        .execute(pool)
        .await;

        match upsert_card {
            Ok(_) => result.cards_upserted += 1,
            Err(e) => {
                warn!("Failed to upsert card {}: {}", row.scryfall_id, e);
                result.errors.push(format!(
                    "Card upsert failed for '{}' ({}): {}",
                    row.name, row.scryfall_id, e
                ));
                continue;
            }
        }

        // Normalize purchase price — 0.0 from ManaBox means unknown
        let acquisition_cost = row
            .purchase_price
            .filter(|&p| p > 0.0);

        let currency = row
            .purchase_price_currency
            .unwrap_or_else(|| "USD".into());

        // Upsert inventory_lots — on conflict ADD quantities (re-import safe)
        let upsert_lot = sqlx::query(
            r#"
            INSERT INTO inventory_lots
                (scryfall_id, foil, condition, quantity, acquisition_cost,
                 acquisition_currency, manabox_id, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(scryfall_id, foil, condition) DO UPDATE SET
                quantity             = quantity + excluded.quantity,
                acquisition_cost     = COALESCE(acquisition_cost, excluded.acquisition_cost),
                manabox_id           = COALESCE(manabox_id, excluded.manabox_id),
                updated_at           = excluded.updated_at
            "#,
        )
        .bind(&row.scryfall_id)
        .bind(&row.foil)
        .bind(&row.condition)
        .bind(row.quantity)
        .bind(acquisition_cost)
        .bind(&currency)
        .bind(row.manabox_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await;

        match upsert_lot {
            Ok(_) => {
                result.lots_upserted += 1;
                info!("Upserted lot: {} x{}", row.name, row.quantity);
            }
            Err(e) => {
                result.errors.push(format!(
                    "Lot upsert failed for '{}': {}",
                    row.name, e
                ));
            }
        }
    }

    Ok(result)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
