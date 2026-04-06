-- Sealed product inventory (packs, boxes, precon/play decks)
CREATE TABLE IF NOT EXISTS sealed_products (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    product_type         TEXT NOT NULL,  -- 'pack' | 'booster_box' | 'precon_deck' | 'play_deck' | 'bundle' | 'other'
    name                 TEXT NOT NULL,
    set_code             TEXT NOT NULL DEFAULT '',
    set_name             TEXT NOT NULL DEFAULT '',
    language             TEXT NOT NULL DEFAULT 'en',
    quantity             INTEGER NOT NULL DEFAULT 1,
    acquisition_cost     REAL,
    acquisition_currency TEXT NOT NULL DEFAULT 'USD',
    notes                TEXT,
    location_id          INTEGER REFERENCES storage_locations(id),
    created_at           INTEGER NOT NULL,
    updated_at           INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_sealed_set  ON sealed_products(set_code);
CREATE INDEX IF NOT EXISTS idx_sealed_type ON sealed_products(product_type);

-- Link invoice items to sealed products (for future deduction)
ALTER TABLE transaction_items ADD COLUMN sealed_id INTEGER REFERENCES sealed_products(id);
