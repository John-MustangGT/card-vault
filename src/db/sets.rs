/// Set release dates, cached from the Scryfall sets API (one request for all sets).
/// Used to order the inventory set filter chronologically.

use anyhow::Result;
use serde::Deserialize;
use sqlx::SqlitePool;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

#[derive(Deserialize)]
struct SetList {
    data: Vec<ScryfallSet>,
}

#[derive(Deserialize)]
struct ScryfallSet {
    code: String,
    name: String,
    released_at: Option<String>,
}

/// Don't hit the API more than once per this many seconds, even when sets are missing.
const MIN_RETRY_SECS: i64 = 600;
static LAST_ATTEMPT: AtomicI64 = AtomicI64::new(0);

/// Fetch set metadata if any set used by inventory has no cached release date.
pub async fn ensure_sets(pool: &SqlitePool) -> Result<()> {
    let missing: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT LOWER(sc.set_code))
         FROM inventory_lots il
         JOIN scryfall_cards sc ON sc.scryfall_id = il.scryfall_id
         WHERE sc.set_code != ''
           AND LOWER(sc.set_code) NOT IN (SELECT code FROM scryfall_sets)",
    )
    .fetch_one(pool)
    .await?;
    if missing == 0 {
        return Ok(());
    }

    let now = unix_now();
    let last = LAST_ATTEMPT.load(Ordering::Relaxed);
    if now - last < MIN_RETRY_SECS {
        return Ok(());
    }
    LAST_ATTEMPT.store(now, Ordering::Relaxed);

    let client = reqwest::Client::builder()
        .user_agent("card-vault/1.0 (collection tracker)")
        .build()?;
    let list: SetList = client
        .get("https://api.scryfall.com/sets")
        .header("Accept", "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut tx = pool.begin().await?;
    for s in &list.data {
        sqlx::query(
            "INSERT INTO scryfall_sets (code, name, released_at, fetched_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(code) DO UPDATE SET
                 name = excluded.name,
                 released_at = excluded.released_at,
                 fetched_at = excluded.fetched_at",
        )
        .bind(s.code.to_lowercase())
        .bind(&s.name)
        .bind(&s.released_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    info!("cached release dates for {} sets", list.data.len());
    Ok(())
}

/// Fire-and-forget wrapper for use from request handlers and startup.
pub fn spawn_ensure_sets(pool: SqlitePool) {
    tokio::spawn(async move {
        if let Err(e) = ensure_sets(&pool).await {
            warn!("set release date fetch failed: {}", e);
        }
    });
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
