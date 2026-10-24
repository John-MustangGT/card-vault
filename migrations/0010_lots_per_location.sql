-- A lot is now identified by (card, finish, condition, location) instead of
-- (card, finish, condition), so the same card can be held in several locations
-- (e.g. one "chaos warehouse" location per import).
--
-- SQLite cannot drop a table-level UNIQUE constraint, so the table is rebuilt.
-- Row ids are preserved, and the lot references held by listings /
-- transaction_items are parked in temp tables and restored afterwards, so the
-- old table can be dropped without tripping foreign key enforcement.
CREATE TEMP TABLE _listing_lots AS SELECT id, lot_id FROM listings WHERE lot_id IS NOT NULL;
CREATE TEMP TABLE _item_lots    AS SELECT id, lot_id FROM transaction_items WHERE lot_id IS NOT NULL;
UPDATE listings SET lot_id = NULL WHERE lot_id IS NOT NULL;
UPDATE transaction_items SET lot_id = NULL WHERE lot_id IS NOT NULL;

CREATE TABLE inventory_lots_new (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    scryfall_id             TEXT NOT NULL REFERENCES scryfall_cards(scryfall_id),
    foil                    TEXT NOT NULL DEFAULT 'normal',
    condition               TEXT NOT NULL DEFAULT 'near_mint',
    quantity                INTEGER NOT NULL DEFAULT 0,
    acquisition_cost        REAL,
    acquisition_currency    TEXT NOT NULL DEFAULT 'USD',
    manabox_id              INTEGER,
    location_id             INTEGER REFERENCES storage_locations(id),
    location_slot           TEXT,
    tags                    TEXT,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    shop_price              REAL,
    notes                   TEXT
);

INSERT INTO inventory_lots_new
    (id, scryfall_id, foil, condition, quantity, acquisition_cost, acquisition_currency,
     manabox_id, location_id, location_slot, tags, created_at, updated_at, shop_price, notes)
SELECT
     id, scryfall_id, foil, condition, quantity, acquisition_cost, acquisition_currency,
     manabox_id, location_id, location_slot, tags, created_at, updated_at, shop_price, notes
FROM inventory_lots;

DROP TABLE inventory_lots;
ALTER TABLE inventory_lots_new RENAME TO inventory_lots;

CREATE INDEX IF NOT EXISTS idx_lots_scryfall ON inventory_lots(scryfall_id);
CREATE INDEX IF NOT EXISTS idx_lots_location ON inventory_lots(location_id);

-- COALESCE so that "no location" (NULL) still counts as one distinct location.
CREATE UNIQUE INDEX IF NOT EXISTS idx_lots_identity
    ON inventory_lots(scryfall_id, foil, condition, COALESCE(location_id, 0));

UPDATE listings
   SET lot_id = (SELECT lot_id FROM _listing_lots t WHERE t.id = listings.id)
 WHERE id IN (SELECT id FROM _listing_lots);
UPDATE transaction_items
   SET lot_id = (SELECT lot_id FROM _item_lots t WHERE t.id = transaction_items.id)
 WHERE id IN (SELECT id FROM _item_lots);
DROP TABLE _listing_lots;
DROP TABLE _item_lots;

-- Set release dates (from the Scryfall sets API) used to order the set filter.
CREATE TABLE IF NOT EXISTS scryfall_sets (
    code        TEXT PRIMARY KEY,   -- lowercase Scryfall set code
    name        TEXT NOT NULL,
    released_at TEXT,               -- 'YYYY-MM-DD'
    fetched_at  INTEGER NOT NULL
);
