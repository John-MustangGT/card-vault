/// Scryfall bulk data import — scans data/*.json.gz, stream-parses JSON array,
/// batch-upserts scryfall_bulk_cards and bulk_prices, skips already-imported files.

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::{
    fs,
    io::BufReader,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use tracing::{info, warn};

// ── Scryfall card shape (only fields we care about) ─────────────────────────

#[derive(Debug, Deserialize)]
struct BulkCard {
    id: String,
    name: String,
    set: String,
    set_name: String,
    collector_number: String,
    lang: String,
    rarity: String,
    #[serde(default)]
    type_line: String,
    #[serde(default)]
    mana_cost: String,
    #[serde(default)]
    cmc: Option<f64>,
    #[serde(default)]
    image_uris: Option<ImageUris>,
    #[serde(default)]
    prices: BulkPrices,
}

#[derive(Debug, Deserialize, Default)]
struct ImageUris {
    normal: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct BulkPrices {
    usd: Option<String>,
    usd_foil: Option<String>,
    usd_etched: Option<String>,
    eur: Option<String>,
    eur_foil: Option<String>,
    tix: Option<String>,
}

fn parse_price(s: &Option<String>) -> Option<f64> {
    s.as_deref().and_then(|v| v.parse().ok())
}

// ── Public entry point ───────────────────────────────────────────────────────

/// Scan `data_dir` for *.json.gz files not yet in scryfall_bulk_imports,
/// process each one, and return the number of new files imported.
pub async fn run_import(pool: &SqlitePool, data_dir: &str) -> Result<u32> {
    let dir = PathBuf::from(data_dir);
    if !dir.exists() {
        warn!("bulk import: data dir {:?} does not exist", dir);
        return Ok(0);
    }

    // Collect .json.gz files, newest first
    let mut gz_files: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.to_str()
                .map(|s| s.ends_with(".json.gz"))
                .unwrap_or(false)
        })
        .collect();
    gz_files.sort();
    gz_files.reverse();

    // Already-imported filenames
    let imported: Vec<String> = sqlx::query_scalar("SELECT filename FROM scryfall_bulk_imports")
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    let mut new_files = 0u32;
    for path in &gz_files {
        let fname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if imported.contains(&fname) {
            info!("bulk import: skip already-imported {}", fname);
            continue;
        }
        match import_file(pool, path, &fname).await {
            Ok(count) => {
                info!("bulk import: {} → {} cards", fname, count);
                new_files += 1;
            }
            Err(e) => {
                warn!("bulk import: error processing {}: {}", fname, e);
            }
        }
    }

    // Prune old bulk_prices — keep rows belonging to the last 10 imports
    let _ = sqlx::query(
        "DELETE FROM bulk_prices WHERE import_id NOT IN (
             SELECT id FROM scryfall_bulk_imports ORDER BY id DESC LIMIT 10)",
    )
    .execute(pool)
    .await;

    Ok(new_files)
}

/// Parse a single .json.gz file and upsert into the DB.
async fn import_file(pool: &SqlitePool, path: &PathBuf, fname: &str) -> Result<usize> {
    let start = Instant::now();
    let now = unix_now();

    info!("bulk import: starting {}", fname);

    // Register import row first (get the import_id)
    let import_id: i64 = sqlx::query_scalar(
        "INSERT INTO scryfall_bulk_imports (filename, cards_processed, imported_at, duration_secs)
         VALUES (?, 0, ?, 0) RETURNING id",
    )
    .bind(fname)
    .bind(now)
    .fetch_one(pool)
    .await
    .context("insert import row")?;

    // Stream-parse the gzipped JSON array
    let file = fs::File::open(path).context("open gz file")?;
    let gz = GzDecoder::new(BufReader::new(file));
    let cards: Vec<BulkCard> = serde_json::from_reader(gz).context("parse json")?;

    let total = cards.len();
    info!("bulk import: {} — {} total cards, filtering to EN", fname, total);

    let en_cards: Vec<BulkCard> = cards.into_iter().filter(|c| c.lang == "en").collect();
    let en_count = en_cards.len();
    info!("bulk import: {} EN cards to upsert", en_count);

    // Batch upsert in chunks of 500
    const BATCH: usize = 500;
    let mut processed = 0usize;
    for chunk in en_cards.chunks(BATCH) {
        let mut tx = pool.begin().await?;
        for card in chunk {
            let image_uri = card
                .image_uris
                .as_ref()
                .and_then(|u| u.normal.clone());

            sqlx::query(
                "INSERT INTO scryfall_bulk_cards
                     (scryfall_id, name, set_code, set_name, collector_number, lang, rarity, type_line, mana_cost, cmc, image_uri, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(scryfall_id) DO UPDATE SET
                     name=excluded.name, set_code=excluded.set_code, set_name=excluded.set_name,
                     collector_number=excluded.collector_number, rarity=excluded.rarity,
                     type_line=excluded.type_line, mana_cost=excluded.mana_cost, cmc=excluded.cmc,
                     image_uri=COALESCE(excluded.image_uri, scryfall_bulk_cards.image_uri),
                     updated_at=excluded.updated_at",
            )
            .bind(&card.id)
            .bind(&card.name)
            .bind(&card.set)
            .bind(&card.set_name)
            .bind(&card.collector_number)
            .bind(&card.lang)
            .bind(&card.rarity)
            .bind(&card.type_line)
            .bind(&card.mana_cost)
            .bind(card.cmc)
            .bind(&image_uri)
            .bind(now)
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "INSERT INTO bulk_prices
                     (scryfall_id, import_id, price_usd, price_usd_foil, price_usd_etched, price_eur, price_eur_foil, price_tix)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(scryfall_id, import_id) DO NOTHING",
            )
            .bind(&card.id)
            .bind(import_id)
            .bind(parse_price(&card.prices.usd))
            .bind(parse_price(&card.prices.usd_foil))
            .bind(parse_price(&card.prices.usd_etched))
            .bind(parse_price(&card.prices.eur))
            .bind(parse_price(&card.prices.eur_foil))
            .bind(parse_price(&card.prices.tix))
            .execute(&mut *tx)
            .await?;

            // Also backfill image_uri on scryfall_cards where NULL
            if let Some(ref uri) = image_uri {
                sqlx::query(
                    "UPDATE scryfall_cards SET image_uri = ? WHERE scryfall_id = ? AND image_uri IS NULL",
                )
                .bind(uri)
                .bind(&card.id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        processed += chunk.len();
        if processed % 10000 == 0 {
            info!("bulk import: {} / {} EN cards processed", processed, en_count);
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    sqlx::query(
        "UPDATE scryfall_bulk_imports SET cards_processed = ?, duration_secs = ? WHERE id = ?",
    )
    .bind(en_count as i64)
    .bind(elapsed)
    .bind(import_id)
    .execute(pool)
    .await?;

    info!(
        "bulk import: {} done — {} EN cards in {:.1}s",
        fname, en_count, elapsed
    );
    Ok(en_count)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
