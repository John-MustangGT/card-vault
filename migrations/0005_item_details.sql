-- Per-item set code and condition for invoice line items
ALTER TABLE transaction_items ADD COLUMN set_code  TEXT;
ALTER TABLE transaction_items ADD COLUMN condition TEXT;
