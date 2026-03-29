use anyhow::{Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Default, serde::Serialize)]
pub struct ImportResult {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
enum CsvFormat {
    ManaBox,
    CardSphere,
}

#[derive(Debug, Clone)]
struct CsvRow {
    scryfall_id: String,
    name: String,
    set_name: Option<String>,
    foil: bool,
    quantity: i64,
    condition: String,
    cost_basis_cents: Option<i64>,
}

fn normalize_condition(raw: &str) -> String {
    match raw.trim().to_uppercase().as_str() {
        "NM" | "NEAR MINT" | "MINT" => "NM".to_string(),
        "LP" | "LIGHT PLAY" | "LIGHTLY PLAYED" | "EX" | "EXCELLENT" => "LP".to_string(),
        "MP" | "MODERATE PLAY" | "MODERATELY PLAYED" | "VG" | "VERY GOOD" => "MP".to_string(),
        "HP" | "HEAVY PLAY" | "HEAVILY PLAYED" | "GD" | "GOOD" => "HP".to_string(),
        "DMG" | "DAMAGED" | "PO" | "POOR" => "DMG".to_string(),
        "PLD" | "PLAYED" => "PLD".to_string(),
        other => {
            if !other.is_empty() {
                other.to_string()
            } else {
                "NM".to_string()
            }
        }
    }
}

fn parse_foil(raw: &str) -> bool {
    matches!(
        raw.trim().to_lowercase().as_str(),
        "true" | "1" | "yes" | "foil" | "etched"
    )
}

pub async fn import_csv(
    pool: &SqlitePool,
    data: &[u8],
    conflict_strategy: &str,
) -> Result<ImportResult> {
    let mut result = ImportResult::default();

    let mut rdr = csv::Reader::from_reader(data);
    let headers = rdr.headers().context("Failed to read CSV headers")?.clone();

    let header_names: Vec<&str> = headers.iter().collect();

    // Detect format
    let has_language = header_names.iter().any(|h| h.eq_ignore_ascii_case("language"));
    let has_scryfall_id = header_names
        .iter()
        .any(|h| h.eq_ignore_ascii_case("scryfall id") || h.eq_ignore_ascii_case("scryfall_id"));

    let format = if has_language {
        CsvFormat::ManaBox
    } else {
        CsvFormat::CardSphere
    };

    if !has_scryfall_id {
        return Err(anyhow::anyhow!(
            "CSV must have a 'Scryfall ID' column"
        ));
    }

    // Find column indices
    let find_col = |names: &[&str]| -> Option<usize> {
        for name in names {
            if let Some(i) = header_names
                .iter()
                .position(|h| h.eq_ignore_ascii_case(name))
            {
                return Some(i);
            }
        }
        None
    };

    let idx_scryfall_id = find_col(&["scryfall id", "scryfall_id"]).ok_or_else(|| {
        anyhow::anyhow!("Missing 'Scryfall ID' column")
    })?;
    let idx_name = find_col(&["name"]);
    let idx_edition = find_col(&["edition", "set name", "set_name"]);
    let idx_foil = find_col(&["foil"]);
    let idx_quantity = find_col(&["quantity", "qty", "count"]);
    let idx_condition = find_col(&["condition"]);
    let idx_price = match format {
        CsvFormat::ManaBox => find_col(&["purchased price", "purchase price", "cost"]),
        CsvFormat::CardSphere => find_col(&["price", "purchased price"]),
    };

    let mut rows: Vec<CsvRow> = Vec::new();

    for record in rdr.records() {
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                result.errors.push(format!("CSV parse error: {e}"));
                continue;
            }
        };

        let get = |idx: Option<usize>| -> &str {
            idx.and_then(|i| record.get(i)).unwrap_or("").trim()
        };

        let scryfall_id = get(Some(idx_scryfall_id)).to_string();
        if scryfall_id.is_empty() {
            result.skipped += 1;
            continue;
        }

        let name = get(idx_name).to_string();
        let set_name = {
            let s = get(idx_edition).to_string();
            if s.is_empty() { None } else { Some(s) }
        };
        let foil = parse_foil(get(idx_foil));
        let quantity: i64 = get(idx_quantity).parse().unwrap_or(1).max(1);
        let condition = normalize_condition(get(idx_condition));

        let cost_basis_cents: Option<i64> = idx_price.and_then(|i| {
            let raw = record.get(i).unwrap_or("").trim();
            let cleaned = raw.trim_start_matches('$').replace(',', "");
            cleaned.parse::<f64>().ok().map(|v| (v * 100.0).round() as i64)
        });

        rows.push(CsvRow {
            scryfall_id,
            name,
            set_name,
            foil,
            quantity,
            condition,
            cost_basis_cents,
        });
    }

    for row in rows {
        // Upsert into scryfall_cards
        let upsert_card_result = sqlx::query(
            "INSERT INTO scryfall_cards (scryfall_id, name, set_name)
             VALUES (?, ?, ?)
             ON CONFLICT(scryfall_id) DO UPDATE SET
               name = COALESCE(excluded.name, name),
               set_name = COALESCE(excluded.set_name, set_name)"
        )
        .bind(&row.scryfall_id)
        .bind(&row.name)
        .bind(&row.set_name)
        .execute(pool)
        .await;

        if let Err(e) = upsert_card_result {
            result.errors.push(format!("Card upsert error for {}: {e}", row.scryfall_id));
            result.skipped += 1;
            continue;
        }

        // Check if lot exists
        let existing: Option<(i64, i64)> = sqlx::query_as(
            "SELECT id, quantity FROM inventory_lots WHERE scryfall_id = ? AND foil = ? AND condition = ?"
        )
        .bind(&row.scryfall_id)
        .bind(row.foil as i64)
        .bind(&row.condition)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);

        match existing {
            Some((lot_id, existing_qty)) => {
                match conflict_strategy {
                    "skip" => {
                        result.skipped += 1;
                    }
                    "replace" => {
                        sqlx::query(
                            "UPDATE inventory_lots SET quantity = ?, updated_at = datetime('now') WHERE id = ?"
                        )
                        .bind(row.quantity)
                        .bind(lot_id)
                        .execute(pool)
                        .await?;
                        result.updated += 1;
                    }
                    _ => {
                        // accumulate (default)
                        sqlx::query(
                            "UPDATE inventory_lots SET quantity = ?, updated_at = datetime('now') WHERE id = ?"
                        )
                        .bind(existing_qty + row.quantity)
                        .bind(lot_id)
                        .execute(pool)
                        .await?;
                        result.updated += 1;
                    }
                }
            }
            None => {
                sqlx::query(
                    "INSERT INTO inventory_lots (scryfall_id, foil, condition, quantity, cost_basis_cents)
                     VALUES (?, ?, ?, ?, ?)"
                )
                .bind(&row.scryfall_id)
                .bind(row.foil as i64)
                .bind(&row.condition)
                .bind(row.quantity)
                .bind(row.cost_basis_cents)
                .execute(pool)
                .await?;
                result.imported += 1;
            }
        }
    }

    Ok(result)
}
