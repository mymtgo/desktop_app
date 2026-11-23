-- League self-management (spec 2026-10-15, sections 3 and 10).
--
-- Player input: no projection, rebuild or league sync writes these columns,
-- so live still equals rebuild. The league sync only moves them with a row's
-- identity (a hand-over, a carry to an imported row).
--
-- matches.no_league: the player took the match out of every league. League
-- fold inputs leave it out; its league_id stays NULL. Setting it clears
-- league_linked in the same statement, and setting league_linked clears it
-- (the baseline CHECK forbids league_linked = 1 without a league_id).
-- matches.edited_at: unix ms of the player's last delete, restore, move or
-- remove from league of this match; NULL: never edited.
ALTER TABLE matches ADD COLUMN no_league INTEGER NOT NULL DEFAULT 0
    CHECK (no_league IN (0, 1));
ALTER TABLE matches ADD COLUMN edited_at INTEGER;

-- leagues.removed_at: the player deleted the league; the row stays as the
-- tombstone and every read skips it (listed_leagues).
-- leagues.user_state: the player's status, NULL for Automatic. Reads show
-- COALESCE(user_state, state). Only 'dropped' reaches the fold (with
-- user_dropped_at); 'active', 'complete' and 'partial' are read-time only.
-- leagues.edited_at: unix ms of the player's last delete, restore or status
-- change of this league (not notes: notes_edited_at keeps its own rule).
ALTER TABLE leagues ADD COLUMN removed_at INTEGER;
ALTER TABLE leagues ADD COLUMN user_state TEXT
    CHECK (user_state IS NULL OR user_state IN ('active', 'complete', 'partial', 'dropped'));
ALTER TABLE leagues ADD COLUMN edited_at INTEGER;

-- Every read that lists, counts or shows leagues reads this view, as match
-- reads use listed_matches. A match whose league_id points at a removed
-- league is a non-league match to every read.
CREATE VIEW listed_leagues AS
SELECT * FROM leagues WHERE removed_at IS NULL;

-- Every existing drop stays a drop: from now on the fold reads
-- user_dropped_at only where user_state = 'dropped' (spec 3.2, section 10).
UPDATE leagues SET user_state = 'dropped' WHERE user_dropped_at IS NOT NULL AND state = 'dropped';
