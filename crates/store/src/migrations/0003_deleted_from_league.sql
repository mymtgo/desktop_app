-- The league a deleted match was in (league self-management spec, section
-- 13).
--
-- matches.deleted_from_league_id: the league row the match was in when it
-- was hidden; NULL when it was in none, or while it is not hidden. Restore,
-- the Deleted list and a league's Restore and Undo read it instead of
-- working the league out afterwards (the league sync clears a hidden log
-- match's league_id, since hidden matches leave the fold).
--
-- Not synced: each device records its own row when it applies the hide.
-- The league sync never writes it, except where a row's identity moves (a
-- hand-over, a carry to an imported row, a live match taking a copy's
-- place), which move it to the surviving row.
ALTER TABLE matches ADD COLUMN deleted_from_league_id INTEGER
    REFERENCES leagues (id) ON DELETE SET NULL;

-- Existing rows, once: a hidden match with a league_id was in that league.
-- Any other hidden match keeps NULL and restores as a match in no league.
-- (No release has migration 0002, so no shipped database has a league
-- deleted together with its matches to recover.)
UPDATE matches SET deleted_from_league_id = league_id
WHERE hidden_at IS NOT NULL AND league_id IS NOT NULL;

-- One rule for every write path (a delete, "Delete them too", a pulled
-- hidden_at, a website tombstone, an import, a test fixture): when hidden_at
-- goes from NULL to a time, record the row's league_id; when it goes back to
-- NULL, clear it. A hidden row hidden again (a newer pulled time) keeps it.
-- Write order: a write path that changes the league and the hide together
-- sets the league first, then hidden_at, so the new league is recorded.
CREATE TRIGGER matches_hidden_records_league
AFTER UPDATE OF hidden_at ON matches
WHEN OLD.hidden_at IS NULL AND NEW.hidden_at IS NOT NULL
BEGIN
    UPDATE matches SET deleted_from_league_id = NEW.league_id WHERE id = NEW.id;
END;

CREATE TRIGGER matches_restored_clears_league
AFTER UPDATE OF hidden_at ON matches
WHEN OLD.hidden_at IS NOT NULL AND NEW.hidden_at IS NULL
BEGIN
    UPDATE matches SET deleted_from_league_id = NULL WHERE id = NEW.id;
END;

CREATE TRIGGER matches_inserted_hidden_records_league
AFTER INSERT ON matches
WHEN NEW.hidden_at IS NOT NULL
BEGIN
    UPDATE matches SET deleted_from_league_id = NEW.league_id WHERE id = NEW.id;
END;

-- A hidden match given a league (a pulled link, a hand-over of its link)
-- was in that league from then on. A cleared league_id (the league sync
-- leaves hidden matches out of the fold) changes nothing.
CREATE TRIGGER matches_hidden_relinked_records_league
AFTER UPDATE OF league_id ON matches
WHEN NEW.hidden_at IS NOT NULL AND NEW.league_id IS NOT NULL
    AND NEW.league_id IS NOT OLD.league_id
BEGIN
    UPDATE matches SET deleted_from_league_id = NEW.league_id WHERE id = NEW.id;
END;
