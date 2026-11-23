-- Endstep captures (endstep support spec): each capture file the browser
-- extension's helper wrote, kept whole (zstd of the file's bytes), one row
-- per capture unit. Facts about a file, not about a match: plan 2 folds them
-- in (started_at, counter) order. A resent unit with a new hash replaces its
-- row.
CREATE TABLE endstep_captures (
    unit_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('match', 'session')),
    started_at INTEGER NOT NULL,
    counter INTEGER NOT NULL,
    format INTEGER NOT NULL,
    extension TEXT NOT NULL,
    browser TEXT NOT NULL,
    developer INTEGER NOT NULL CHECK (developer IN (0, 1)),
    classification TEXT NOT NULL CHECK (classification IN ('human', 'bot', 'unknown')),
    frame_count INTEGER NOT NULL,
    file_hash TEXT NOT NULL,
    body BLOB NOT NULL,
    imported_at INTEGER NOT NULL
) STRICT;
CREATE INDEX endstep_captures_fold_order ON endstep_captures (started_at, counter);
