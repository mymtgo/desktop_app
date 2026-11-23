-- MTGO's own verdict on a match (#454).
--
-- matches.history_outcome: the outcome MTGO's game history records for a
-- log match (its MatchWinners and MatchLosers), 'win' or 'loss'. NULL until
-- the history names the local player. While it is set, the projection's
-- upsert writes it as the outcome instead of the derived one, so a rebuild
-- keeps MTGO's answer where the live log could not tell (a player who
-- leaves at a level score logs only "Match Tied").
ALTER TABLE matches ADD COLUMN history_outcome TEXT
    CHECK (history_outcome IS NULL OR history_outcome IN ('win', 'loss'));
