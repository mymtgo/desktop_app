-- Endstep sync bookkeeping (endstep plan 2): each capture's sync version
-- (0 until the Endstep sync folds it; raising ENDSTEP_SYNC_VERSION refolds
-- every unit), the refs each unit carries (game:, db:, lobby: and deck: ids,
-- so a unit arriving late finds its match), and each ranked Endstep match's
-- rating change.
ALTER TABLE endstep_captures ADD COLUMN synced_version INTEGER NOT NULL DEFAULT 0;
CREATE TABLE endstep_unit_refs (
    unit_id TEXT NOT NULL REFERENCES endstep_captures(unit_id) ON DELETE CASCADE,
    ref TEXT NOT NULL,
    PRIMARY KEY (unit_id, ref)
) STRICT;
CREATE INDEX endstep_unit_refs_ref ON endstep_unit_refs (ref);
CREATE TABLE endstep_ranked_results (
    match_id INTEGER PRIMARY KEY REFERENCES matches(id) ON DELETE CASCADE,
    status TEXT,
    format_id TEXT,
    rating_before INTEGER,
    rating_after INTEGER,
    score_before INTEGER,
    score_after INTEGER,
    delta INTEGER,
    rd_before INTEGER,
    rd_after INTEGER,
    rank TEXT
) STRICT;
