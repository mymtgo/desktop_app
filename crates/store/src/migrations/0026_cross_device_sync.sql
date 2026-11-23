-- Cross-device sync of Arena and Endstep (cross-device sync spec, sections
-- 6 to 8; #776).
--
-- synced_sources: the raw source of a match pulled from another device, the
-- bundle's `sources` section in canonical form, zstd. Never read by a local
-- fold; gone with its match, and deleted when a live recording takes the
-- match over. league_client_id: the run the bundle named, so a pulled Arena
-- limited match joins its course's run in the run fold even when the run
-- arrives after it.
CREATE TABLE synced_sources (
    match_id INTEGER PRIMARY KEY REFERENCES matches (id) ON DELETE CASCADE,
    platform TEXT NOT NULL CHECK (platform IN ('arena', 'endstep')),
    payload BLOB NOT NULL,
    league_client_id TEXT
) STRICT;

-- synced_arena_runs: an Arena course run as another device last pushed it,
-- one row per account and course. An input to the Arena run fold, never a
-- league row of its own. player_fields is the bundle's player fields, JSON.
CREATE TABLE synced_arena_runs (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    course_id TEXT NOT NULL,
    event_name TEXT NOT NULL,
    kind TEXT NOT NULL,
    wins INTEGER NOT NULL,
    losses INTEGER NOT NULL,
    state TEXT NOT NULL,
    observed_at INTEGER NOT NULL,
    player_fields TEXT,
    PRIMARY KEY (account_id, course_id)
) STRICT;

-- An Arena course run's record: the newest snapshot of its course known
-- here, local or pulled (wins, losses and when it was observed; its state is
-- the run's). Written by the Arena run sync; NULL for every other run.
ALTER TABLE leagues ADD COLUMN course_wins INTEGER;
ALTER TABLE leagues ADD COLUMN course_losses INTEGER;
ALTER TABLE leagues ADD COLUMN course_observed_at INTEGER;

-- An Arena course run (`arena:<course id>`, no second `:`) has the same
-- client id on every device: `arena_<course id>`. The Arena run sync writes
-- it on insert. An orphan run (`arena:<event>:<first token>`) keeps the id
-- the trigger gave it; it is never synced. Arena runs were never synced
-- before, so no server copy names the old ids.
UPDATE leagues SET sync_client_id = 'arena_' || substr(run_key, 7)
WHERE run_key LIKE 'arena:%' AND instr(substr(run_key, 7), ':') = 0;

-- The 0004 triggers never fill an Arena run: the run sync owns its id.
-- SQLite cannot alter a trigger, so both are created again, unchanged but
-- for `run_key NOT LIKE 'arena:%'`.
DROP TRIGGER leagues_inserted_fills_sync_client_id;
CREATE TRIGGER leagues_inserted_fills_sync_client_id
AFTER INSERT ON leagues
WHEN NEW.sync_client_id IS NULL AND NEW.token IS NOT NULL AND NEW.run_key NOT LIKE 'arena:%'
BEGIN
    UPDATE leagues SET sync_client_id = NEW.token || '_' || strftime('%Y%m%dT%H%M%SZ',
        (NEW.started_at - (NEW.started_at % 1000 + 1000) % 1000) / 1000, 'unixepoch')
    WHERE id = NEW.id;
END;

DROP TRIGGER leagues_token_set_fills_sync_client_id;
CREATE TRIGGER leagues_token_set_fills_sync_client_id
AFTER UPDATE OF token ON leagues
WHEN NEW.sync_client_id IS NULL AND NEW.token IS NOT NULL AND NEW.run_key NOT LIKE 'arena:%'
BEGIN
    UPDATE leagues SET sync_client_id = NEW.token || '_' || strftime('%Y%m%dT%H%M%SZ',
        (NEW.started_at - (NEW.started_at % 1000 + 1000) % 1000) / 1000, 'unixepoch')
    WHERE id = NEW.id;
END;

-- decks.pulled_platform: the platform of an Arena or Endstep deck pulled
-- from another device before this device had an account of its platform
-- (the deck lands with no account). A deck without an account is otherwise
-- an MTGO deck, so the MTGO adoption rules (first login, `Adopt`) and the
-- filing of a pulled match skip a deck whose pulled_platform names another
-- platform. Kept once the deck is filed (its account then says the same).
ALTER TABLE decks ADD COLUMN pulled_platform TEXT
    CHECK (pulled_platform IN ('arena', 'endstep'));
