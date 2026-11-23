
-- Cards and decks (target schema section 7, decks spec sections 4 to 6).

-- One row per MTGO catalog id (a printing). A stub (only catalog_id, maybe a
-- name from the game log or MTGO's token XML) is written when an id is
-- first referenced (`store::cards`); the cards worker fills it from the API
-- and the 0.x import writes full rows. scryfall_id IS NULL means "not
-- resolved yet". local_image and local_art_crop name the files the cards
-- worker downloaded into v1's images folder (`{id}.jpg`, `{id}_art.jpg`);
-- NULL means "show the remote URL".
CREATE TABLE cards (
    catalog_id INTEGER PRIMARY KEY,
    scryfall_id TEXT,
    oracle_id TEXT,
    name TEXT,
    type TEXT,
    sub_type TEXT,
    rarity TEXT,
    color_identity TEXT,
    colors TEXT,
    cmc REAL,
    mana_cost TEXT,
    set_name TEXT,
    set_code TEXT,
    image TEXT,
    art_crop TEXT,
    local_image TEXT,
    local_art_crop TEXT
) STRICT;
-- Card stats group by oracle; the Cards page groups printings by oracle.
CREATE INDEX cards_oracle_id ON cards (oracle_id) WHERE oracle_id IS NOT NULL;

-- A deck read from MTGO's deck files (or imported from 0.x). It is never
-- deleted: archiving sets `archived_at`.
CREATE TABLE decks (
    id INTEGER PRIMARY KEY,
    mtgo_id TEXT NOT NULL UNIQUE,
    account_id INTEGER REFERENCES accounts (id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    original_name TEXT,
    format TEXT NOT NULL,
    color_identity TEXT,
    color_identity_manual INTEGER NOT NULL DEFAULT 0 CHECK (color_identity_manual IN (0, 1)),
    cover_catalog_id INTEGER,
    archived_at INTEGER,
    file_synced_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    source TEXT NOT NULL CHECK (source IN ('file', 'imported')),
    -- The mirror of the server's deck slot ledger (07 spec section 6), written
    -- only by the slot rules and the deck sync toggle, never by the deck file
    -- sync, and never touching updated_at.
    cloud_sync_enabled INTEGER NOT NULL DEFAULT 0 CHECK (cloud_sync_enabled IN (0, 1)),
    -- Input: the streamer's name for the deck on the league card (overlay
    -- spec 4, 0.x `overlay_deck_labels`), written only by the deck edit
    -- (`DeckEdit::SetOverlayLabel`, trimmed, at most 40 characters), never
    -- touching updated_at. Not in the deck bundle (0.x never synced it).
    overlay_label TEXT CHECK (overlay_label IS NULL OR length(overlay_label) BETWEEN 1 AND 40),
    -- Input (archetypes spec section 4): set by hand or in bulk; the deck
    -- sync's prefill writes it only while it is NULL, as 0.x. The table is
    -- in archetypes.sql (a forward reference, which SQLite allows).
    archetype_id INTEGER REFERENCES archetypes (id) ON DELETE SET NULL
) STRICT;

CREATE INDEX decks_account_id ON decks (account_id);
CREATE INDEX decks_archetype_id ON decks (archetype_id) WHERE archetype_id IS NOT NULL;

-- The unique (deck_id, signature) index also serves lookups by deck_id.
CREATE TABLE deck_versions (
    id INTEGER PRIMARY KEY,
    deck_id INTEGER NOT NULL REFERENCES decks (id) ON DELETE CASCADE,
    signature TEXT NOT NULL,
    modified_at INTEGER NOT NULL,
    UNIQUE (deck_id, signature)
) STRICT;
