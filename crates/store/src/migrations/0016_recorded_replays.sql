-- Recorded MTGO replays (recorded replays spec): each .mtgr file the opt-in
-- recorder wrote, kept whole as text, one row per file keyed by the match
-- token it carries (lowercased). Facts about a file, not about a match: a
-- recording may arrive before its match exists, so there is no match column.
-- A match links to its recording at read time (the token, else the MTGO match
-- id), and a game to its recorded game (the MTGO game id, else the game
-- number), so live equals rebuild.
CREATE TABLE recorded_replays (
    token TEXT PRIMARY KEY,
    mtgo_match_id INTEGER,
    text TEXT NOT NULL,
    file_hash TEXT NOT NULL,
    recorder TEXT,
    imported_at INTEGER NOT NULL
) STRICT;
CREATE TABLE recorded_replay_games (
    token TEXT NOT NULL REFERENCES recorded_replays (token) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    game_number INTEGER NOT NULL,
    mtgo_game_id INTEGER,
    PRIMARY KEY (token, position)
) STRICT;
CREATE INDEX recorded_replays_mtgo_match_id ON recorded_replays (mtgo_match_id) WHERE mtgo_match_id IS NOT NULL;
