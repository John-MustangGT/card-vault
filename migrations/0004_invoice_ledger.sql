-- Invoice number (YYYYMMDD-mmm format), shipping_charged flag
ALTER TABLE transactions ADD COLUMN invoice_id TEXT;
ALTER TABLE transactions ADD COLUMN shipping_charged INTEGER NOT NULL DEFAULT 1;

-- Expense ledger: postage, supplies, mileage, fees, etc.
CREATE TABLE IF NOT EXISTS ledger_entries (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    entry_date  TEXT    NOT NULL,  -- YYYY-MM-DD
    category    TEXT    NOT NULL,  -- 'postage' | 'supplies' | 'mileage' | 'fees' | 'other'
    description TEXT    NOT NULL,
    amount      REAL    NOT NULL,  -- positive = expense (money out)
    notes       TEXT,
    created_at  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_ledger_date     ON ledger_entries(entry_date DESC);
CREATE INDEX IF NOT EXISTS idx_ledger_category ON ledger_entries(category);
