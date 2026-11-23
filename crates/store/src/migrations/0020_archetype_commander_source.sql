-- Commander archetypes (Arena Brawl): archetypes.source gains 'commander'.
-- SQLite cannot change a CHECK, so the table is rebuilt with every column,
-- default, check, index and row id kept. Foreign keys are off while
-- migrations run, so the tables that reference archetypes (and the table's
-- own merged_into_id) keep their rows; the migration's foreign key check
-- confirms it.
CREATE TABLE archetypes_new (
    id INTEGER PRIMARY KEY,
    uuid TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    format TEXT,
    color_identity TEXT,
    source TEXT NOT NULL CHECK (source IN ('downloaded', 'manual', 'fallback', 'commander')),
    definition TEXT CHECK (definition IS NULL OR json_valid(definition)),
    definition_version TEXT,
    retired_at INTEGER,
    merged_into_id INTEGER REFERENCES archetypes (id) ON DELETE SET NULL,
    source_match_id INTEGER REFERENCES matches (id) ON DELETE SET NULL,
    incomplete INTEGER NOT NULL DEFAULT 0 CHECK (incomplete IN (0, 1)),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    offline_held INTEGER NOT NULL DEFAULT 0 CHECK (offline_held IN (0, 1)),
    CHECK ((source = 'fallback') = (format IS NULL)),
    CHECK (source = 'downloaded' OR definition IS NULL),
    CHECK (merged_into_id IS NULL OR merged_into_id <> id)
) STRICT;

INSERT INTO archetypes_new (
    id, uuid, name, format, color_identity, source, definition, definition_version,
    retired_at, merged_into_id, source_match_id, incomplete, created_at, updated_at,
    offline_held
)
SELECT
    id, uuid, name, format, color_identity, source, definition, definition_version,
    retired_at, merged_into_id, source_match_id, incomplete, created_at, updated_at,
    offline_held
FROM archetypes;

DROP TABLE archetypes;
ALTER TABLE archetypes_new RENAME TO archetypes;
CREATE INDEX archetypes_format ON archetypes (format, name);
