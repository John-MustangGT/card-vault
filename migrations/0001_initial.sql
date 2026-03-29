CREATE TABLE IF NOT EXISTS scryfall_cards (
    scryfall_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    set_code TEXT,
    set_name TEXT,
    collector_number TEXT,
    image_uri TEXT,
    mana_cost TEXT,
    type_line TEXT,
    rarity TEXT,
    current_price_usd REAL,
    current_price_usd_foil REAL,
    price_updated_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS storage_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    type TEXT NOT NULL CHECK(type IN ('binder','box','other')),
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS inventory_lots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scryfall_id TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    foil INTEGER NOT NULL DEFAULT 0,
    condition TEXT NOT NULL DEFAULT 'NM',
    quantity INTEGER NOT NULL DEFAULT 1,
    cost_basis_cents INTEGER,
    location_id INTEGER REFERENCES storage_locations(id),
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(scryfall_id, foil, condition)
);

CREATE TABLE IF NOT EXISTS individual_cards (
    id TEXT PRIMARY KEY,
    lot_id INTEGER REFERENCES inventory_lots(id),
    scryfall_id TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    status TEXT NOT NULL DEFAULT 'in_stock' CHECK(status IN ('in_stock','listed','out_for_grading','graded','sold')),
    foil INTEGER NOT NULL DEFAULT 0,
    condition TEXT NOT NULL DEFAULT 'NM',
    cost_basis_cents INTEGER,
    location_id INTEGER REFERENCES storage_locations(id),
    scan_front_path TEXT,
    scan_back_path TEXT,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS price_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    scryfall_id TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    price_usd REAL,
    price_usd_foil REAL,
    recorded_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS listings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    individual_card_id TEXT REFERENCES individual_cards(id),
    lot_id INTEGER REFERENCES inventory_lots(id),
    platform TEXT NOT NULL,
    listing_price_cents INTEGER,
    listed_at TEXT,
    sold_at TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS grading_submissions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    individual_card_id TEXT NOT NULL REFERENCES individual_cards(id),
    service TEXT NOT NULL,
    submitted_at TEXT,
    returned_at TEXT,
    grade TEXT,
    cert_number TEXT,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    type TEXT NOT NULL CHECK(type IN ('buy','sell','trade_in','trade_out')),
    platform TEXT,
    counterparty TEXT,
    total_cents INTEGER,
    notes TEXT,
    transacted_at TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS transaction_items (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id INTEGER NOT NULL REFERENCES transactions(id),
    individual_card_id TEXT REFERENCES individual_cards(id),
    lot_id INTEGER REFERENCES inventory_lots(id),
    quantity INTEGER NOT NULL DEFAULT 1,
    price_cents INTEGER,
    notes TEXT
);
