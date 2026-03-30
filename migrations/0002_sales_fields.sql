-- Separate buyer address into components for label generation
ALTER TABLE transactions ADD COLUMN buyer_city    TEXT;
ALTER TABLE transactions ADD COLUMN buyer_state   TEXT;
ALTER TABLE transactions ADD COLUMN buyer_zip     TEXT;

-- Free-text description line for transaction items
ALTER TABLE transaction_items ADD COLUMN description TEXT;
