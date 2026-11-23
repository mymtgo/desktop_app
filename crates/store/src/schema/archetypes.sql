-- Archetypes (archetypes spec section 4, target schema section 10).

-- One archetype v1 knows. source: 'downloaded' (the server's: name, format,
-- colours and definition come from the API and are replaced by the change
-- check), 'manual' (the player's, input; uuid is the device prefix plus a
-- uuid, as 0.x), 'fallback' (the two seeds, Homebrew and Rogue, format NULL).
-- definition is the server's detection rules and key list as JSON, with
-- definition_version; NULL on a downloaded row known only by uuid (a sync
-- pull or a 0.x import), which the change check fills. retired_at: the
-- server retired it; matches keep it, detection and pickers skip it.
-- merged_into_id: the player's merge (0.x), followed one hop by detection.
CREATE TABLE archetypes (
    id INTEGER PRIMARY KEY,
    uuid TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    format TEXT,
    color_identity TEXT,
    source TEXT NOT NULL CHECK (source IN ('downloaded', 'manual', 'fallback')),
    definition TEXT CHECK (definition IS NULL OR json_valid(definition)),
    definition_version TEXT,
    retired_at INTEGER,
    merged_into_id INTEGER REFERENCES archetypes (id) ON DELETE SET NULL,
    source_match_id INTEGER REFERENCES matches (id) ON DELETE SET NULL,
    incomplete INTEGER NOT NULL DEFAULT 0 CHECK (incomplete IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    CHECK ((source = 'fallback') = (format IS NULL)),
    CHECK (source = 'downloaded' OR definition IS NULL),
    CHECK (merged_into_id IS NULL OR merged_into_id <> id)
) STRICT;
CREATE INDEX archetypes_format ON archetypes (format, name);

-- The fallbacks every database has (0.x seed migration): Homebrew for an
-- unidentified opponent, Rogue beside it.
INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at) VALUES
    ('00000000-0000-0000-0000-000000000001', 'Homebrew', NULL, 'fallback', 0, 0),
    ('00000000-0000-0000-0000-000000000002', 'Rogue', NULL, 'fallback', 0, 0);

-- Card lists. A downloaded archetype has one list (list 1, the definition's
-- key list, written by the change check). A manual archetype has one list
-- per variant (0.x archetype_decks of a manual archetype), 1, 2, ... in
-- creation order. oracle_id is copied from the card row when known.
CREATE TABLE archetype_cards (
    archetype_id INTEGER NOT NULL REFERENCES archetypes (id) ON DELETE CASCADE,
    list INTEGER NOT NULL CHECK (list >= 1),
    catalog_id INTEGER NOT NULL,
    oracle_id TEXT,
    quantity INTEGER NOT NULL CHECK (quantity >= 1),
    sideboard INTEGER NOT NULL CHECK (sideboard IN (0, 1)),
    PRIMARY KEY (archetype_id, list, catalog_id, sideboard)
) STRICT;
-- The Cards page and the per-card archetype sheet look cards up.
CREATE INDEX archetype_cards_catalog_id ON archetype_cards (catalog_id);

-- One row per match and seat (target schema section 10, 1v1). manual = 1 is
-- input (a hand assignment) and nothing derived ever replaces it. manual = 0
-- rows are detection's: the archetypes worker writes them through
-- ArchetypeEdit and replaces them on re-detection.
CREATE TABLE match_archetypes (
    match_id INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    is_opponent INTEGER NOT NULL CHECK (is_opponent IN (0, 1)),
    archetype_id INTEGER NOT NULL REFERENCES archetypes (id) ON DELETE CASCADE,
    confidence REAL NOT NULL CHECK (confidence >= 0 AND confidence <= 1),
    manual INTEGER NOT NULL DEFAULT 0 CHECK (manual IN (0, 1)),
    PRIMARY KEY (match_id, is_opponent)
) STRICT;
CREATE INDEX match_archetypes_archetype_id ON match_archetypes (archetype_id, is_opponent);

-- One sideboard plan per deck and opponent archetype (0.x sideboard_guides).
-- Input: the player's, a 0.x import's or a pulled deck bundle's. updated_at
-- moves on every plan save (0.x touch) and feeds the deck's sync touch key.
CREATE TABLE sideboard_guides (
    id INTEGER PRIMARY KEY,
    deck_id INTEGER NOT NULL REFERENCES decks (id) ON DELETE CASCADE,
    archetype_id INTEGER NOT NULL REFERENCES archetypes (id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (deck_id, archetype_id)
) STRICT;
CREATE INDEX sideboard_guides_archetype_id ON sideboard_guides (archetype_id);

-- The plan's cards (0.x sideboard_guide_cards): copies to bring in from the
-- sideboard or take out of the maindeck, by oracle id (a plan outlives
-- printings, as 0.x).
CREATE TABLE sideboard_guide_cards (
    guide_id INTEGER NOT NULL REFERENCES sideboard_guides (id) ON DELETE CASCADE,
    oracle_id TEXT NOT NULL CHECK (length(oracle_id) BETWEEN 1 AND 64),
    direction TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 99),
    PRIMARY KEY (guide_id, direction, oracle_id)
) STRICT;

-- Free-text notes per deck and opponent archetype (0.x deck_archetype_notes),
-- several per pair, newest first. created_at is content (it travels in the
-- deck bundle), so a pull restores it. AUTOINCREMENT: an id is never
-- reused, so the deck's sync touch key (highest note id) always moves.
CREATE TABLE deck_archetype_notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    deck_id INTEGER NOT NULL REFERENCES decks (id) ON DELETE CASCADE,
    archetype_id INTEGER NOT NULL REFERENCES archetypes (id) ON DELETE CASCADE,
    body TEXT NOT NULL CHECK (length(body) BETWEEN 1 AND 2000),
    created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX deck_archetype_notes_pair ON deck_archetype_notes (deck_id, archetype_id);
CREATE INDEX deck_archetype_notes_archetype_id ON deck_archetype_notes (archetype_id, created_at);
