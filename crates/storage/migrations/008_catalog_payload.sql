-- Store the full synced product payload for MPOS compatibility views.
ALTER TABLE catalog_items ADD COLUMN raw_product_json TEXT;
