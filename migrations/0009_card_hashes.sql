-- Pre-computed perceptual hashes of card art crops for scanner recognition
CREATE TABLE IF NOT EXISTS card_hashes (
    scryfall_id     TEXT PRIMARY KEY REFERENCES scryfall_cards(scryfall_id),
    dhash           TEXT NOT NULL,      -- 64-bit dHash stored as 16-char hex string
    art_crop_url    TEXT,               -- the URL used to compute this hash
    computed_at     INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_card_hashes_computed ON card_hashes(computed_at);
