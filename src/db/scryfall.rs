use sqlx::SqlitePool;
use tracing::{info, warn, error};

pub async fn hydrate_cards(pool: SqlitePool) {
    let client = match reqwest::Client::builder()
        .user_agent("card-vault/0.1 (personal collection tracker)")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to build reqwest client: {e}");
            return;
        }
    };

    loop {
        // Find one card needing hydration
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT scryfall_id FROM scryfall_cards
             WHERE set_code IS NULL OR price_updated_at IS NULL
                OR price_updated_at < datetime('now', '-1 day')
             LIMIT 1"
        )
        .fetch_optional(&pool)
        .await
        .unwrap_or(None);

        let scryfall_id = match row {
            Some((id,)) => id,
            None => {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                continue;
            }
        };

        let url = format!("https://api.scryfall.com/cards/{}", scryfall_id);
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(card) => {
                        let set_code = card["set"].as_str().map(str::to_string);
                        let set_name = card["set_name"].as_str().map(str::to_string);
                        let collector_number = card["collector_number"].as_str().map(str::to_string);
                        let image_uri = card["image_uris"]["normal"]
                            .as_str()
                            .or_else(|| card["image_uris"]["large"].as_str())
                            .map(str::to_string);
                        let mana_cost = card["mana_cost"].as_str().map(str::to_string);
                        let type_line = card["type_line"].as_str().map(str::to_string);
                        let rarity = card["rarity"].as_str().map(str::to_string);
                        let price_usd = card["prices"]["usd"]
                            .as_str()
                            .and_then(|s| s.parse::<f64>().ok());
                        let price_usd_foil = card["prices"]["usd_foil"]
                            .as_str()
                            .and_then(|s| s.parse::<f64>().ok());

                        let update_result = sqlx::query(
                            "UPDATE scryfall_cards SET
                               set_code = ?,
                               set_name = COALESCE(?, set_name),
                               collector_number = ?,
                               image_uri = ?,
                               mana_cost = ?,
                               type_line = ?,
                               rarity = ?,
                               current_price_usd = ?,
                               current_price_usd_foil = ?,
                               price_updated_at = datetime('now')
                             WHERE scryfall_id = ?"
                        )
                        .bind(&set_code)
                        .bind(&set_name)
                        .bind(&collector_number)
                        .bind(&image_uri)
                        .bind(&mana_cost)
                        .bind(&type_line)
                        .bind(&rarity)
                        .bind(price_usd)
                        .bind(price_usd_foil)
                        .bind(&scryfall_id)
                        .execute(&pool)
                        .await;

                        if let Err(e) = update_result {
                            warn!("Failed to update card {scryfall_id}: {e}");
                        } else {
                            // Record price history
                            let _ = sqlx::query(
                                "INSERT INTO price_history (scryfall_id, price_usd, price_usd_foil)
                                 VALUES (?, ?, ?)"
                            )
                            .bind(&scryfall_id)
                            .bind(price_usd)
                            .bind(price_usd_foil)
                            .execute(&pool)
                            .await;

                            info!("Hydrated card {scryfall_id}");
                        }
                    }
                    Err(e) => warn!("Failed to parse Scryfall response for {scryfall_id}: {e}"),
                }
            }
            Ok(resp) => {
                warn!("Scryfall returned {} for {scryfall_id}", resp.status());
                // Mark as attempted to avoid tight loop on 404s
                let _ = sqlx::query(
                    "UPDATE scryfall_cards SET price_updated_at = datetime('now') WHERE scryfall_id = ?"
                )
                .bind(&scryfall_id)
                .execute(&pool)
                .await;
            }
            Err(e) => warn!("Scryfall request failed for {scryfall_id}: {e}"),
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
    }
}
