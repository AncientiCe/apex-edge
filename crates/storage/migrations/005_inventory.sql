-- Inventory availability fields on catalog_items.
-- is_active: persisted from CatalogItem.is_active (default true; false = item not sold).
-- available_qty: nullable; NULL = inventory not tracked (untracked items are always addable).
-- image_urls: JSON array of ordered product image URLs for PDP gallery.
ALTER TABLE catalog_items ADD COLUMN is_active INTEGER NOT NULL DEFAULT 1;
ALTER TABLE catalog_items ADD COLUMN available_qty INTEGER;
ALTER TABLE catalog_items ADD COLUMN image_urls TEXT NOT NULL DEFAULT '[]';
