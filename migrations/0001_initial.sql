-- card-vault schema
-- Migration: 0001_initial

PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

-- Cached Scryfall card data
CREATE TABLE IF NOT EXISTS scryfall_cards (
    scryfall_id         TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    set_code            TEXT NOT NULL,
    set_name            TEXT NOT NULL,
    collector_number    TEXT NOT NULL,
    rarity              TEXT NOT NULL,   -- 'common' | 'uncommon' | 'rare' | 'mythic'
    language            TEXT NOT NULL DEFAULT 'en',
    image_uri           TEXT,
    cached_at           INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_scryfall_name ON scryfall_cards(name);
CREATE INDEX IF NOT EXISTS idx_scryfall_set  ON scryfall_cards(set_code, collector_number);

-- Physical storage locations
CREATE TABLE IF NOT EXISTS storage_locations (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    type        TEXT NOT NULL,      -- 'binder' | 'box'
    name        TEXT NOT NULL,      -- "Binder 3", "Long Box 1"
    description TEXT,
    created_at  INTEGER NOT NULL
);

-- Bulk quantity lots (most cards live here)
CREATE TABLE IF NOT EXISTS inventory_lots (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    scryfall_id             TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    foil                    TEXT NOT NULL DEFAULT 'normal',     -- 'normal' | 'foil' | 'etched'
    condition               TEXT NOT NULL DEFAULT 'near_mint',  -- 'near_mint' | 'lightly_played' | 'moderately_played' | 'heavily_played' | 'damaged'
    quantity                INTEGER NOT NULL DEFAULT 0,
    acquisition_cost        REAL,           -- nullable - many cards have unknown cost basis
    acquisition_currency    TEXT NOT NULL DEFAULT 'USD',
    manabox_id              INTEGER,        -- preserved for reconciliation with ManaBox
    location_id             INTEGER REFERENCES storage_locations(id),
    location_slot           TEXT,           -- freeform: "Page 2 Left", "Row 3", etc.
    tags                    TEXT,           -- JSON array of strings: ["for sale", "trade"]
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    UNIQUE(scryfall_id, foil, condition)
);

CREATE INDEX IF NOT EXISTS idx_lots_scryfall   ON inventory_lots(scryfall_id);
CREATE INDEX IF NOT EXISTS idx_lots_location   ON inventory_lots(location_id);

-- High-value individual card tracking
CREATE TABLE IF NOT EXISTS individual_cards (
    id                      TEXT PRIMARY KEY,   -- base62, 6 chars, printed on Avery 5267 label
    scryfall_id             TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    foil                    TEXT NOT NULL DEFAULT 'normal',
    condition               TEXT NOT NULL,
    acquisition_cost        REAL,
    acquisition_currency    TEXT NOT NULL DEFAULT 'USD',
    status                  TEXT NOT NULL DEFAULT 'in_stock', -- 'in_stock' | 'listed' | 'out_for_grading' | 'graded' | 'sold'
    location_id             INTEGER REFERENCES storage_locations(id),
    location_slot           TEXT,
    scan_front_path         TEXT,   -- relative path: "scans/aB3xK9/front.jpg"
    scan_back_path          TEXT,
    scan_updated_at         INTEGER,
    cert_number             TEXT,   -- PSA/BGS cert after grading
    grade                   REAL,   -- 9.5, 10, etc.
    notes                   TEXT,
    tags                    TEXT,   -- JSON array
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_individual_scryfall ON individual_cards(scryfall_id);
CREATE INDEX IF NOT EXISTS idx_individual_status   ON individual_cards(status);
CREATE INDEX IF NOT EXISTS idx_individual_location ON individual_cards(location_id);

-- Price snapshots (time-series)
CREATE TABLE IF NOT EXISTS price_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    scryfall_id TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    foil        TEXT NOT NULL DEFAULT 'normal',
    source      TEXT NOT NULL DEFAULT 'scryfall',   -- 'scryfall' | 'tcgplayer' | 'cardmarket'
    price_usd   REAL,
    price_eur   REAL,
    scraped_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_price_card_time ON price_history(scryfall_id, scraped_at DESC);

-- Active marketplace listings
CREATE TABLE IF NOT EXISTS listings (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    lot_id              INTEGER REFERENCES inventory_lots(id),
    individual_id       TEXT REFERENCES individual_cards(id),
    platform            TEXT NOT NULL,  -- 'ebay' | 'tcgplayer' | 'cardsphere' | 'direct'
    platform_listing_id TEXT,
    list_price          REAL NOT NULL,
    currency            TEXT NOT NULL DEFAULT 'USD',
    listed_at           INTEGER NOT NULL,
    expires_at          INTEGER,
    status              TEXT NOT NULL DEFAULT 'active',  -- 'active' | 'sold' | 'cancelled' | 'expired'
    updated_at          INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_listings_status ON listings(status);

-- Grading submissions
CREATE TABLE IF NOT EXISTS grading_submissions (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    individual_id   TEXT NOT NULL REFERENCES individual_cards(id),
    service         TEXT NOT NULL,      -- 'PSA' | 'BGS' | 'CGC' | 'SGC'
    submission_id   TEXT,               -- grading company order number
    declared_value  REAL,
    tier            TEXT,               -- 'economy' | 'standard' | 'express'
    submitted_at    INTEGER NOT NULL,
    returned_at     INTEGER,
    grade           REAL,
    cert_number     TEXT,
    created_at      INTEGER NOT NULL
);

-- Sales transactions
CREATE TABLE IF NOT EXISTS transactions (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    buyer_name          TEXT,
    buyer_email         TEXT,
    buyer_address       TEXT,
    platform            TEXT,   -- 'ebay' | 'tcgplayer' | 'cardsphere' | 'direct'
    platform_order_id   TEXT,
    shipping_cost       REAL,
    status              TEXT NOT NULL DEFAULT 'pending',  -- 'pending' | 'shipped' | 'complete'
    tracking_number     TEXT,
    sold_at             INTEGER NOT NULL,
    created_at          INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS transaction_items (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id  INTEGER NOT NULL REFERENCES transactions(id),
    lot_id          INTEGER REFERENCES inventory_lots(id),
    individual_id   TEXT REFERENCES individual_cards(id),
    quantity        INTEGER NOT NULL DEFAULT 1,
    sale_price      REAL NOT NULL,
    currency        TEXT NOT NULL DEFAULT 'USD'
);

CREATE INDEX IF NOT EXISTS idx_tx_items_tx ON transaction_items(transaction_id);
