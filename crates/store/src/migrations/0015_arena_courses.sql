-- Arena course snapshots (Arena limited runs spec section 2): each
-- EventGetCoursesV2 response's limited courses, one row per course, under the
-- account logged in when it was read. Facts: never edited or deleted. A
-- snapshot with no limited course is one marker row with course_id ''.
CREATE TABLE arena_course_observations (
    arena_log_file_id INTEGER NOT NULL REFERENCES arena_log_files (id) ON DELETE RESTRICT,
    byte_offset INTEGER NOT NULL,
    observed_at INTEGER NOT NULL, -- the response header time, UTC
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE RESTRICT,
    course_id TEXT NOT NULL,      -- '' for the marker row of an empty snapshot
    event_name TEXT NOT NULL,
    module TEXT NOT NULL,
    wins INTEGER NOT NULL,
    losses INTEGER NOT NULL,
    deck_id TEXT NOT NULL,
    deck_name TEXT NOT NULL,
    cards TEXT NOT NULL,          -- JSON {main, side, pool}, catalog ids (grpId + ARENA_ID_OFFSET)
    PRIMARY KEY (arena_log_file_id, byte_offset, course_id)
) STRICT;

CREATE INDEX arena_course_observations_account
    ON arena_course_observations (account_id, arena_log_file_id, byte_offset);

-- The event each Arena match was played in, from its room state: matches has
-- no event name, and two events can share a format code.
CREATE TABLE arena_match_events (
    match_token TEXT PRIMARY KEY,
    event_name TEXT NOT NULL
) STRICT;
