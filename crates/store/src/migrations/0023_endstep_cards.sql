-- Endstep card identity (endstep support spec, section 7): Endstep has no
-- card ids of its own, so each card name and printing seen gets a row here,
-- and its catalog id is ENDSTEP_ID_OFFSET (200,000,000) plus the row id.
-- name is the front face exactly as Endstep sends it; set_code is lower
-- case; set_code and collector_number are empty when the printing is
-- unknown.
CREATE TABLE endstep_cards (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    set_code TEXT NOT NULL DEFAULT '',
    collector_number TEXT NOT NULL DEFAULT '',
    first_seen_at INTEGER NOT NULL,
    UNIQUE (name, set_code, collector_number)
) STRICT;
