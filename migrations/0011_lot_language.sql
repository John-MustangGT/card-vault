-- Language is recorded per lot so the physical card's language can be edited
-- without changing the shared scryfall_cards row for its printing.
ALTER TABLE inventory_lots ADD COLUMN language TEXT NOT NULL DEFAULT 'en';

UPDATE inventory_lots
   SET language = (SELECT sc.language FROM scryfall_cards sc WHERE sc.scryfall_id = inventory_lots.scryfall_id);

DROP INDEX IF EXISTS idx_lots_identity;

CREATE UNIQUE INDEX IF NOT EXISTS idx_lots_identity
    ON inventory_lots(scryfall_id, language, foil, condition, COALESCE(location_id, 0));
