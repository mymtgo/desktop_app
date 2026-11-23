-- Arena tracking (2026-11-03 Arena support design, section 7). No table is
-- rebuilt: columns and tables are added.
--
-- accounts.platform and opponents.platform: 'mtgo' for every existing row.
-- An Arena account's username is the login's Name#12345 (MTGO usernames
-- cannot contain '#'); its Arena user id is accounts.arena_user_id. An Arena
-- opponent's username is 'arena:' followed by its user id, its screen name
-- in opponents.display_name.
ALTER TABLE accounts ADD COLUMN platform TEXT NOT NULL DEFAULT 'mtgo' CHECK (platform IN ('mtgo', 'arena'));
ALTER TABLE accounts ADD COLUMN arena_user_id TEXT;
CREATE UNIQUE INDEX accounts_arena_user_id ON accounts (arena_user_id) WHERE arena_user_id IS NOT NULL;
ALTER TABLE opponents ADD COLUMN platform TEXT NOT NULL DEFAULT 'mtgo' CHECK (platform IN ('mtgo', 'arena'));
ALTER TABLE opponents ADD COLUMN display_name TEXT;

-- Arena log ingestion (section 5): facts, never edited or deleted. Hashes
-- are u64 values stored bit for bit as INTEGER.
CREATE TABLE arena_log_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    identity_hash INTEGER NOT NULL,
    head_hash INTEGER NOT NULL,
    head_len INTEGER NOT NULL,
    created_at_fs INTEGER,
    last_observed_size INTEGER NOT NULL DEFAULT 0,
    read_offset INTEGER NOT NULL DEFAULT 0,
    first_seen_at INTEGER NOT NULL,
    sealed_at INTEGER,
    seal_reason TEXT
) STRICT;

-- One log entry can decode into several events: seq is the event's index
-- within its entry. raw is the decoded event as JSON, zstd-compressed when
-- long (compressed = 1).
CREATE TABLE arena_log_events (
    arena_log_file_id INTEGER NOT NULL REFERENCES arena_log_files (id) ON DELETE RESTRICT,
    byte_offset INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN (
        'login', 'authenticated', 'deck_set', 'room_state', 'game_info', 'first_turn',
        'deck_submitted', 'mulligan', 'rank', 'season', 'detailed_logs'
    )),
    match_token TEXT,
    logged_at INTEGER NOT NULL,
    raw BLOB NOT NULL,
    compressed INTEGER NOT NULL DEFAULT 0 CHECK (compressed IN (0, 1)),
    PRIMARY KEY (arena_log_file_id, byte_offset, seq)
) STRICT;

CREATE INDEX arena_log_events_match_token ON arena_log_events (match_token) WHERE match_token IS NOT NULL;

-- Rank (section 8): one reading of one ladder, keyed by where it was read.
CREATE TABLE arena_rank_observations (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE RESTRICT,
    ladder TEXT NOT NULL CHECK (ladder IN ('constructed', 'limited')),
    season INTEGER NOT NULL,
    observed_at INTEGER NOT NULL,
    class TEXT NOT NULL,
    level INTEGER NOT NULL,
    step INTEGER NOT NULL,
    matches_won INTEGER NOT NULL,
    matches_lost INTEGER NOT NULL,
    mythic_percentile REAL,
    mythic_placement INTEGER,
    arena_log_file_id INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL,
    PRIMARY KEY (arena_log_file_id, byte_offset, ladder)
) STRICT;
