
-- Limited (target schema section 9, limited spec section 7): drafts, their
-- picks and the registered decks of limited matches, stored. The league sync
-- writes them from the log; the 0.x import writes its own rows
-- (`source = 'imported'`); a pick note is the player's input on its pick.

-- Drafts, stored (target schema section 9). Written by the league sync from the
-- draft fold (`source = 'log'`); a 0.x draft (`imported`) is input, of which the
-- sync writes only `state` (the 30-minute rule), `abandons_at` and `league_id`,
-- until v1 has events for its token (the row then turns `log`, keeping its id and
-- pick notes). `abandons_at` is when an open draft turns abandoned (the
-- tracker's deadline).
CREATE TABLE drafts (
    id INTEGER PRIMARY KEY,
    draft_token TEXT NOT NULL UNIQUE,
    account_id INTEGER REFERENCES accounts (id) ON DELETE RESTRICT,
    source TEXT NOT NULL CHECK (source IN ('log', 'imported')),
    league_id INTEGER REFERENCES leagues (id) ON DELETE SET NULL,
    league_event_id INTEGER,
    mtgo_course_id INTEGER,
    mtgo_draft_id INTEGER,
    pod_token TEXT,
    seat_count INTEGER NOT NULL DEFAULT 8,
    seat_index INTEGER,
    booster_catalog_id INTEGER,
    state TEXT NOT NULL CHECK (state IN ('connecting', 'picking', 'finished', 'abandoned')),
    pack_size INTEGER NOT NULL DEFAULT 14,
    picks_expected INTEGER NOT NULL DEFAULT 42,
    started_at INTEGER,
    ended_at INTEGER,
    abandons_at INTEGER CHECK (abandons_at IS NULL OR state IN ('connecting', 'picking')),
    detached_at INTEGER,
    -- Input: the 0.x import's. `imported_from` is the 0.x `drafts.id`, or the
    -- next free id when another 0.x database took it; `legacy_draft_id` is the
    -- 0.x `drafts.id` itself (a rerun finds its own draft by token and this);
    -- `imported_league_from` is the `imported_from` of the draft's 0.x run.
    -- A pulled draft holds the synthetic negative id (`pulled_legacy_id`) in
    -- both `imported_from` and `legacy_draft_id`. A draft the log takes over
    -- loses `imported_from` but keeps `legacy_draft_id` and
    -- `imported_league_from` as they were.
    imported_from INTEGER UNIQUE,
    imported_league_from INTEGER,
    legacy_draft_id INTEGER,
    CHECK ((source = 'imported') = (imported_from IS NOT NULL))
) STRICT;
-- A run holds one draft; also the run's draft and the unlinked drafts.
CREATE UNIQUE INDEX drafts_league_id ON drafts (league_id) WHERE league_id IS NOT NULL;
CREATE INDEX drafts_account_id ON drafts (account_id);

-- A draft's picks, by `(draft_id, ordinal)`. `cards_available` is a JSON array
-- of catalog ids; `reservations` a JSON array of `{catalog_id, at_ms}`. `note`
-- is input: the sync never writes it, and a pick the fold no longer makes but
-- that carries a note is detached, not deleted.
CREATE TABLE draft_picks (
    draft_id INTEGER NOT NULL REFERENCES drafts (id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 1),
    pack_number INTEGER NOT NULL,
    pick_number INTEGER NOT NULL,
    pack_id INTEGER,
    direction INTEGER,
    cards_available TEXT NOT NULL,
    picked_catalog_id INTEGER,
    picked_card_id INTEGER,
    reservations TEXT NOT NULL,
    shown_at INTEGER,
    deadline_at INTEGER,
    picked_at INTEGER,
    detached_at INTEGER,
    -- Input: the player's.
    note TEXT CHECK (note IS NULL OR length(note) BETWEEN 1 AND 2000),
    note_updated_at INTEGER,
    CHECK ((note IS NULL) = (note_updated_at IS NULL)),
    PRIMARY KEY (draft_id, ordinal)
) STRICT;

-- The deck MTGO registered for a limited match: `registered` (the league sync's,
-- from the match's newest readable `match_deck_registered` line, with that line's
-- position) or `imported` (0.x's). `cards` is a JSON array of
-- `{catalog_id, quantity, sideboard}`.
CREATE TABLE limited_deck_snapshots (
    match_id INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    source TEXT NOT NULL CHECK (source IN ('registered', 'imported')),
    cards TEXT NOT NULL,
    signature TEXT NOT NULL,
    captured_at INTEGER NOT NULL,
    -- A registered row: the log line it was read from (the sync rereads only when it changes).
    event_file_id INTEGER,
    event_offset INTEGER,
    CHECK ((source = 'registered') = (event_file_id IS NOT NULL AND event_offset IS NOT NULL)),
    PRIMARY KEY (match_id, source)
) STRICT;
