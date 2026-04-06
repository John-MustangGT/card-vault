-- Bulk Scryfall card data (static card info, all EN cards)
CREATE TABLE IF NOT EXISTS scryfall_bulk_cards (
    scryfall_id     TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    set_code        TEXT NOT NULL DEFAULT '',
    set_name        TEXT NOT NULL DEFAULT '',
    collector_number TEXT NOT NULL DEFAULT '',
    lang            TEXT NOT NULL DEFAULT 'en',
    rarity          TEXT NOT NULL DEFAULT '',
    type_line       TEXT NOT NULL DEFAULT '',
    mana_cost       TEXT NOT NULL DEFAULT '',
    cmc             REAL,
    image_uri       TEXT,   -- normal-size image URI
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bulk_cards_name     ON scryfall_bulk_cards(name);
CREATE INDEX IF NOT EXISTS idx_bulk_cards_set_code ON scryfall_bulk_cards(set_code);

-- Price snapshots from each bulk file import
CREATE TABLE IF NOT EXISTS bulk_prices (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    scryfall_id TEXT NOT NULL,
    import_id   INTEGER NOT NULL,
    price_usd   REAL,
    price_usd_foil   REAL,
    price_usd_etched REAL,
    price_eur        REAL,
    price_eur_foil   REAL,
    price_tix        REAL,
    UNIQUE(scryfall_id, import_id)
);

CREATE INDEX IF NOT EXISTS idx_bulk_prices_card   ON bulk_prices(scryfall_id);
CREATE INDEX IF NOT EXISTS idx_bulk_prices_import ON bulk_prices(import_id);

-- Track which bulk files have been imported (by filename, unique)
CREATE TABLE IF NOT EXISTS scryfall_bulk_imports (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    filename        TEXT NOT NULL UNIQUE,
    cards_processed INTEGER NOT NULL DEFAULT 0,
    imported_at     INTEGER NOT NULL,
    duration_secs   REAL NOT NULL DEFAULT 0
);
