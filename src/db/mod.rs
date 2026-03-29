pub mod import;
pub mod scryfall;

use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use anyhow::Result;

pub async fn create_pool(database_url: &str) -> Result<SqlitePool> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;
    Ok(pool)
}

/// Generate a random base62 ID of the given length.
pub fn gen_base62(len: usize) -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| ALPHABET[rng.gen_range(0..62)] as char)
        .collect()
}

/// Generate a unique base62 ID that doesn't already exist in individual_cards.
pub async fn unique_individual_id(pool: &SqlitePool) -> Result<String> {
    for _ in 0..10 {
        let id = gen_base62(6);
        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM individual_cards WHERE id = ?")
            .bind(&id)
            .fetch_one(pool)
            .await
            .unwrap_or(false);
        if !exists {
            return Ok(id);
        }
    }
    Err(anyhow::anyhow!("Failed to generate unique ID after 10 attempts"))
}
