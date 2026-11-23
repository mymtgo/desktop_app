-- Leagues and tournaments (target schema section 8, leagues spec sections 3
-- to 6).

-- League runs, stored (target schema section 8). Written by the league sync
-- (`league_sync`) from the fold, except the input columns (`user_dropped_at`,
-- `notes`, `imported_from`, `imported_seed`) and manual rows, which no
-- derivation writes. `run_key` is the identity the sync matches fold output
-- by: `{league token}:{first match token}` (log), `draft:{draft token}`,
-- `0x:{0.x id}` (imported), `manual:{id}` (manual). `source = 'imported'`
-- covers both a 0.x import and a league pulled from the cloud (the `0x:` key
-- is only a key). A run the fold no longer
-- makes but that carries input is detached (`detached_at`), not deleted.
-- Manual rows (`source = 'manual'`) are the player's: made by the league edit
-- (`id` = MAX(id) + 1, so `run_key` is written once) or the 0.x import, with
-- the deck's format and newest version when made, and 0.x's `manual_` token
-- (`manual_` and 24 random letters and digits; the 0.x import keeps 0.x's),
-- which its sync client id needs (07 ruling 13). Members: a match linked by
-- hand or by the 0.x import has `matches.league_id` with `league_linked = 1`;
-- every other match's `league_id` is the sync's.
CREATE TABLE leagues (
    id INTEGER PRIMARY KEY,
    run_key TEXT NOT NULL UNIQUE,
    account_id INTEGER REFERENCES accounts (id) ON DELETE RESTRICT,
    source TEXT NOT NULL CHECK (source IN ('log', 'imported', 'manual')),
    kind TEXT NOT NULL CHECK (kind IN ('constructed', 'draft', 'sealed')),
    state TEXT NOT NULL CHECK (state IN ('active', 'complete', 'partial', 'dropped')),
    token TEXT,
    event_id INTEGER,
    mtgo_course_id INTEGER,
    name TEXT NOT NULL,
    format TEXT,
    set_code TEXT,
    deck_version_id INTEGER REFERENCES deck_versions (id) ON DELETE SET NULL,
    deck_id INTEGER REFERENCES decks (id) ON DELETE RESTRICT,
    started_at INTEGER NOT NULL,
    joined_at INTEGER,
    completed_at INTEGER,
    dropped_at INTEGER,
    created_at INTEGER NOT NULL,
    detached_at INTEGER,
    -- Input: the player's.
    user_dropped_at INTEGER,
    notes TEXT CHECK (notes IS NULL OR length(notes) BETWEEN 1 AND 5000),
    -- When the player last set or cleared the note in v1 (unix ms); NULL for
    -- a note the 0.x import brought or none. The league sync's carry reads it
    -- (a v1 note beats an imported one; the newest v1 note wins).
    notes_edited_at INTEGER,
    -- Input: the 0.x import's (the 0.x `leagues.id`; the run as 0.x left it, JSON).
    imported_from INTEGER UNIQUE,
    imported_seed TEXT CHECK (imported_seed IS NULL OR json_valid(imported_seed)),
    CHECK (source <> 'manual' OR (run_key LIKE 'manual:%' AND kind = 'constructed'
        AND state = 'complete' AND token IS NOT NULL
        AND substr(token, 1, 7) = 'manual_')),
    CHECK ((source = 'imported') = (imported_seed IS NOT NULL)),
    CHECK (source <> 'imported' OR run_key = '0x:' || imported_from),
    CHECK (source <> 'log' OR (token IS NOT NULL AND imported_from IS NULL))
) STRICT;
CREATE INDEX leagues_account_kind ON leagues (account_id, kind, started_at);
CREATE INDEX leagues_deck_id ON leagues (deck_id) WHERE deck_id IS NOT NULL;
CREATE INDEX leagues_deck_version_id ON leagues (deck_version_id) WHERE deck_version_id IS NOT NULL;

-- One row per tournament of an account's matches (`matches.tournament_event_id`),
-- written by the league sync. `token` is the `tournament_token` of its first
-- match that has one. `name` is synthesized (and rewritten by the sync)
-- while `name_synthesized = 1`, until the API supplies the real one (07).
-- `name_checked_at` is when the API was asked (a 200 or a 404); only the API
-- worker's tournament names write it, never the league sync.
CREATE TABLE tournaments (
    id INTEGER PRIMARY KEY,
    mtgo_event_id INTEGER NOT NULL UNIQUE,
    account_id INTEGER REFERENCES accounts (id) ON DELETE RESTRICT,
    token TEXT,
    name TEXT NOT NULL,
    format TEXT,
    started_at INTEGER NOT NULL,
    name_synthesized INTEGER NOT NULL DEFAULT 1 CHECK (name_synthesized IN (0, 1)),
    name_checked_at INTEGER
) STRICT;
