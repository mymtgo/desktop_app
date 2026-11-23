CREATE TABLE arena_rank_gap_claims (
    match_token TEXT PRIMARY KEY,
    arena_log_file_id INTEGER NOT NULL,
    byte_offset INTEGER NOT NULL,
    claimed_at INTEGER NOT NULL
) STRICT;
