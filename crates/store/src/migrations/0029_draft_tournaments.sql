-- A draft played in a tournament rather than a league (a Swiss Draft):
-- drafts.tournament_id is the tournament the league sync placed it in, and
-- drafts.tournament_name the event's name its EventSyncData_t line gives
-- (`Reality Fracture Swiss Draft`). Derived like drafts.league_id: the sync
-- writes both and a draft holds a league or a tournament, never both. No
-- backfill: the league fold version raise places existing drafts.
ALTER TABLE drafts ADD COLUMN tournament_id INTEGER
    REFERENCES tournaments (id) ON DELETE SET NULL;
ALTER TABLE drafts ADD COLUMN tournament_name TEXT;
-- A tournament holds one draft per account (two accounts can play one
-- event); also the Limited page's tournament drafts.
CREATE UNIQUE INDEX drafts_tournament_id ON drafts (account_id, tournament_id)
    WHERE tournament_id IS NOT NULL;
