CREATE TABLE IF NOT EXISTS uid_pool (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    uid        TEXT    NOT NULL UNIQUE,
    used       INTEGER NOT NULL DEFAULT 0,
    card_id    TEXT    REFERENCES individual_cards(id),
    created_at INTEGER NOT NULL,
    used_at    INTEGER
);

CREATE INDEX IF NOT EXISTS idx_uid_pool_used ON uid_pool(used, id);
