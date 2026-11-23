-- Endstep (2026-11-10 Endstep support design, section 7): accounts.platform
-- and opponents.platform also take 'endstep'. SQLite cannot change a CHECK,
-- so both tables are rebuilt by its procedure (the first rebuild in this
-- list): a new table with every column, default, CHECK and UNIQUE the old
-- one has, in the same order, the rows copied with their ids, the old table
-- dropped and the new one renamed. Migrations run with foreign keys off and
-- the runner checks them before it commits, so the tables that reference
-- these ones keep pointing at the same ids.
--
-- The view match_rank_readings (0013) joins accounts: a rename fails while a
-- view names a dropped table, so it is dropped first and created again,
-- word for word, at the end. No other view or trigger names these tables.
DROP VIEW match_rank_readings;

CREATE TABLE accounts_new (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    login_id INTEGER UNIQUE,
    first_seen_at INTEGER NOT NULL,
    tracked INTEGER NOT NULL DEFAULT 1 CHECK (tracked IN (0, 1)),
    tracked_chosen INTEGER NOT NULL DEFAULT 0 CHECK (tracked_chosen IN (0, 1)),
    -- Derived: the log time of the newest Login line that named the account
    -- (the current account rule, release spec 5), kept so the pages read no
    -- log events.
    last_login_at INTEGER,
    platform TEXT NOT NULL DEFAULT 'mtgo' CHECK (platform IN ('mtgo', 'arena', 'endstep')),
    arena_user_id TEXT,
    display_name TEXT
) STRICT;
INSERT INTO accounts_new (id, username, login_id, first_seen_at, tracked, tracked_chosen,
    last_login_at, platform, arena_user_id, display_name)
SELECT id, username, login_id, first_seen_at, tracked, tracked_chosen,
    last_login_at, platform, arena_user_id, display_name
FROM accounts;
DROP TABLE accounts;
ALTER TABLE accounts_new RENAME TO accounts;
CREATE UNIQUE INDEX accounts_arena_user_id ON accounts (arena_user_id) WHERE arena_user_id IS NOT NULL;

CREATE TABLE opponents_new (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    first_seen_at INTEGER NOT NULL,
    platform TEXT NOT NULL DEFAULT 'mtgo' CHECK (platform IN ('mtgo', 'arena', 'endstep')),
    display_name TEXT
) STRICT;
INSERT INTO opponents_new (id, username, first_seen_at, platform, display_name)
SELECT id, username, first_seen_at, platform, display_name
FROM opponents;
DROP TABLE opponents;
ALTER TABLE opponents_new RENAME TO opponents;

-- Word for word from 0013_match_rank.sql.
CREATE VIEW match_rank_readings AS
SELECT m.id AS match_id, r.arena_log_file_id AS rank_file_id, r.byte_offset AS rank_byte_offset
FROM matches m
JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
LEFT JOIN arena_log_events f ON f.rowid = (
    SELECT e.rowid FROM arena_log_events e
    WHERE e.match_token = m.token
    ORDER BY e.arena_log_file_id, e.byte_offset
    LIMIT 1)
JOIN arena_rank_observations r ON r.rowid = (
    SELECT o.rowid FROM arena_rank_observations o
    WHERE o.account_id = m.account_id AND o.ladder = 'constructed'
      AND CASE WHEN m.source = 'manual' THEN o.observed_at < m.started_at
          ELSE (o.arena_log_file_id, o.byte_offset) < (f.arena_log_file_id, f.byte_offset)
          END
    ORDER BY o.season DESC, o.arena_log_file_id DESC, o.byte_offset DESC
    LIMIT 1)
WHERE m.match_type = 'ranked';
