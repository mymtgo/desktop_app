-- The event overlay's tournaments (event overlay spec 7.1). Per device:
-- never synced between devices, and never made from a pulled or imported
-- match (only from this device's log). A 48-hour window over the log, not
-- history: no backfill, so these are bare tables.

-- One row per tournament the account joined, written by the tournament
-- sync (tournament_sync) from the standing fold (leagues::standing). Derived:
-- rows may be deleted and folded again; an unregistered event has no row.
-- The record is not stored (it comes from the account's matches at read
-- time, plus byes), nor any estimate, nor another player's name or login
-- id. Times are log times (logged_at), never write times.
-- rounds_read_file_id, rounds_read_offset and rounds_login_id are the
-- sync's bookkeeping: the newest log position whose round lines are decoded
-- into tournament_rounds (so a refold never parses a round info line
-- twice), and the account login id they were decoded for (the account's
-- pairing is found by it; another login id decodes them again).
CREATE TABLE tournament_standings (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    mtgo_event_id INTEGER NOT NULL,
    token TEXT NOT NULL,
    name TEXT,
    format TEXT,
    cut_size INTEGER,
    swiss_rounds INTEGER,
    round_minutes INTEGER,
    starts_at INTEGER,
    status TEXT NOT NULL CHECK (status IN (
        'registered', 'playing', 'waiting_for_round', 'bye', 'made_cut', 'finished'
    )),
    round INTEGER,
    stage TEXT NOT NULL CHECK (stage IN ('swiss', 'top8', 'semifinal', 'final')),
    in_playoffs INTEGER NOT NULL DEFAULT 0 CHECK (in_playoffs IN (0, 1)),
    round_started_at INTEGER,
    round_result_at INTEGER,
    rank INTEGER,
    players INTEGER,
    matches_left INTEGER,
    byes INTEGER NOT NULL DEFAULT 0,
    finish TEXT CHECK (finish IN (
        'trophied', 'top_cut', 'missed_cut', 'complete', 'dropped', 'eliminated'
    )),
    place INTEGER,
    finished_at INTEGER,
    last_transition_at INTEGER NOT NULL,
    last_event_at INTEGER NOT NULL,
    rounds_read_file_id INTEGER,
    rounds_read_offset INTEGER,
    rounds_login_id INTEGER,
    PRIMARY KEY (account_id, mtgo_event_id),
    CHECK ((status = 'finished') = (finish IS NOT NULL)),
    CHECK ((rounds_read_file_id IS NULL) = (rounds_read_offset IS NULL))
) STRICT;

-- Each round of a joined tournament as its round info line decoded: every
-- match token of the round (a JSON array), byes, the account's pairing, the
-- round's match start and length, and the log position and time of the
-- line, so a refold puts it back in log order without parsing the 220 KB
-- line again. Facts, not fold output: the stage and the account's result
-- come from the fold, which reads the round results from log_events each
-- time (they carry the tournament token, so compaction never moves them).
-- The earliest line of a round wins, as in the fold. Written by the
-- tournament sync, deleted with its standing.
CREATE TABLE tournament_rounds (
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    mtgo_event_id INTEGER NOT NULL,
    round INTEGER NOT NULL CHECK (round >= 1),
    bye_count INTEGER NOT NULL DEFAULT 0,
    match_tokens TEXT NOT NULL DEFAULT '[]',
    account_match_token TEXT,
    account_bye INTEGER NOT NULL DEFAULT 0 CHECK (account_bye IN (0, 1)),
    started_at INTEGER,
    match_minutes INTEGER,
    paired_file_id INTEGER NOT NULL,
    paired_offset INTEGER NOT NULL,
    paired_at INTEGER NOT NULL,
    PRIMARY KEY (account_id, mtgo_event_id, round)
) STRICT;

-- FlsPlayerEventStateChangedMessage facts (the InPlayoffs flag) of every
-- event, joined or not (a few lines per session; the fold reads them only
-- for joined ones), keyed by log position, so a re-read of a file writes
-- nothing new. Not a log_events kind: that would rebuild the largest table.
CREATE TABLE tournament_player_states (
    log_file_id INTEGER NOT NULL REFERENCES log_files (id) ON DELETE RESTRICT,
    byte_offset INTEGER NOT NULL,
    tournament_token TEXT NOT NULL,
    new_state INTEGER NOT NULL,
    in_playoffs INTEGER NOT NULL CHECK (in_playoffs IN (0, 1)),
    logged_at INTEGER NOT NULL,
    PRIMARY KEY (log_file_id, byte_offset)
) STRICT;

CREATE INDEX tournament_player_states_token
    ON tournament_player_states (tournament_token, log_file_id, byte_offset);

-- The sync finds joined tournaments by their state changes in the window:
-- without this it would range-scan every tournament line, 220 KB round info
-- lines included. A partial index: a few lines per event.
CREATE INDEX log_events_tournament_state_changes
    ON log_events (logged_at) WHERE kind = 'tournament_state_changed';

-- The event card's last fallback deck is the account's last played deck of
-- the tournament's format (D9): one index seek per format, however many
-- matches the account has.
CREATE INDEX matches_account_format_started
    ON matches (account_id, format, started_at) WHERE deck_version_id IS NOT NULL;
