use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::SqlitePool;
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

#[derive(Debug, Default)]
pub struct ImportResult {
    pub rows_processed: usize,
    pub cards_upserted: usize,
    pub lots_upserted: usize,
    pub errors: Vec<String>,
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
