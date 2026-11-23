CREATE TABLE arena_game_messages (
    arena_log_file_id INTEGER NOT NULL REFERENCES arena_log_files (id) ON DELETE RESTRICT,
    byte_offset INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    match_token TEXT NOT NULL,
    game_number INTEGER,
    source TEXT NOT NULL CHECK (source IN ('gre', 'client')),
    logged_at INTEGER NOT NULL,
    raw BLOB NOT NULL,
    compressed INTEGER NOT NULL DEFAULT 0 CHECK (compressed IN (0, 1)),
    PRIMARY KEY (arena_log_file_id, byte_offset, seq)
) STRICT;

CREATE INDEX arena_game_messages_match ON arena_game_messages (match_token);

CREATE TABLE arena_game_archives (
    match_token TEXT PRIMARY KEY,
    messages BLOB NOT NULL,
    message_count INTEGER NOT NULL,
    archived_at INTEGER NOT NULL
) STRICT;

CREATE TABLE arena_game_syncs (
    match_id INTEGER PRIMARY KEY REFERENCES matches (id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK (status IN ('derived', 'no_messages')),
    derived_version INTEGER NOT NULL,
    synced_at INTEGER NOT NULL
) STRICT;
