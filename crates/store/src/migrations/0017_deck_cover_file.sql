-- A deck's own cover image: decks.cover_file names a file v1 wrote in the app
-- data folder (`deck-covers/`). Local only: never synced, so a pulled deck
-- leaves it alone. The cover card (cover_catalog_id) stays as the public
-- fallback.
ALTER TABLE decks ADD COLUMN cover_file TEXT;
