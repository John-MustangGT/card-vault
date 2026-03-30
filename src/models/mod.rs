use sqlx::FromRow;

#[allow(dead_code)]
#[derive(Debug, FromRow)]
pub struct ScryfallCard {
    pub scryfall_id: String,
    pub name: String,
    pub set_code: String,
    pub set_name: String,
    pub collector_number: String,
    pub rarity: String,
    pub language: String,
    pub image_uri: Option<String>,
    pub cached_at: i64,
}

#[allow(dead_code)]
#[derive(Debug, FromRow)]
pub struct InventoryLot {
    pub id: i64,
    pub scryfall_id: String,
    pub foil: String,
    pub condition: String,
    pub quantity: i64,
    pub acquisition_cost: Option<f64>,
    pub acquisition_currency: String,
    pub manabox_id: Option<i64>,
    pub location_id: Option<i64>,
    pub location_slot: Option<String>,
    pub tags: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
