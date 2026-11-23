-- Sync identity and download cursor (sync identity and cursor spec,
-- sections 2 and 3; #431, #392).
--
-- leagues.sync_client_id: the league's sync client id, the identity its
-- bundle and its member matches' bundles carry. Filled once and never
-- recomputed: a rekey (its first match deleted, so started_at moves) keeps
-- it, so the league keeps one server copy. Existing rows and new rows with
-- a token get the 0.x formula, `sync_bundles::league_client_id(token,
-- started_at)`: `{token}_{start as UTC Ymd\THis\Z}`. A league pulled from
-- another device stores the bundle's id as given, which can disagree with
-- the formula (that device may have rekeyed its run). NULL while the row
-- has no token (an imported row can have none).
--
-- sync_state.download_cursor: the server's download cursor from the last
-- finished run, stored exactly as received; NULL before the first one, or
-- with a server too old to send one. The next manifest call sends it.
--
-- The start is floored to whole seconds, as the Rust formula does (chrono
-- floors; SQLite's integer `/` truncates toward zero, which differs only
-- for a start before 1970 with milliseconds).
ALTER TABLE leagues ADD COLUMN sync_client_id TEXT;

UPDATE leagues SET sync_client_id = token || '_' || strftime('%Y%m%dT%H%M%SZ',
    (started_at - (started_at % 1000 + 1000) % 1000) / 1000, 'unixepoch')
WHERE token IS NOT NULL;

-- Not unique: the token-and-start rule can match more than one row, and the
-- lookups keep their own tie-breaks.
CREATE INDEX leagues_sync_client_id ON leagues (sync_client_id);

-- Every insert path: a row inserted with a token and no id gets the formula.
-- A row inserted with an id (a pulled league) keeps it.
CREATE TRIGGER leagues_inserted_fills_sync_client_id
AFTER INSERT ON leagues
WHEN NEW.sync_client_id IS NULL AND NEW.token IS NOT NULL
BEGIN
    UPDATE leagues SET sync_client_id = NEW.token || '_' || strftime('%Y%m%dT%H%M%SZ',
        (NEW.started_at - (NEW.started_at % 1000 + 1000) % 1000) / 1000, 'unixepoch')
    WHERE id = NEW.id;
END;

-- A row given its first token later gets the formula then, once.
CREATE TRIGGER leagues_token_set_fills_sync_client_id
AFTER UPDATE OF token ON leagues
WHEN NEW.sync_client_id IS NULL AND NEW.token IS NOT NULL
BEGIN
    UPDATE leagues SET sync_client_id = NEW.token || '_' || strftime('%Y%m%dT%H%M%SZ',
        (NEW.started_at - (NEW.started_at % 1000 + 1000) % 1000) / 1000, 'unixepoch')
    WHERE id = NEW.id;
END;

ALTER TABLE sync_state ADD COLUMN download_cursor TEXT;
