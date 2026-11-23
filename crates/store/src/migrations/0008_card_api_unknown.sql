-- Card ids the API does not know (#501).
--
-- cards.api_unknown_at: when (UTC ms) a lookup answered without this
-- unresolved card. Population passes over it until the recheck window has
-- passed, so an id the API will never know is not asked about every run.
-- NULL for a card never answered so, and cleared once it resolves.
ALTER TABLE cards ADD COLUMN api_unknown_at INTEGER;
