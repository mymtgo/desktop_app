-- Baseline schema (target schema spec section 3): one file per domain,
-- concatenated in this order into the single baseline migration. Edited in
-- place until the first live beta; development databases are deleted, and
-- `open_writer` refuses one made by another build.
--
-- Every foreign key states its delete rule (foundation spec 4.1 rule 5).
-- Facts and registries are never deleted, so their references RESTRICT.

-- Log ingestion (target schema section 4): facts, never edited or deleted.

CREATE TABLE log_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    identity_hash TEXT NOT NULL,
    head_hash TEXT NOT NULL,
    head_len INTEGER NOT NULL,
    created_at_fs INTEGER,
    anchor_offset INTEGER,
    anchor_hash TEXT,
    day_anchor TEXT NOT NULL,
    timezone TEXT NOT NULL,
    inherited_username TEXT,
    local_username TEXT,
    local_login_id INTEGER,
    cursor_offset INTEGER NOT NULL DEFAULT 0,
    clock_date TEXT NOT NULL,
    clock_last_secs INTEGER,
    last_observed_size INTEGER NOT NULL DEFAULT 0,
    stuck_ticks INTEGER NOT NULL DEFAULT 0,
    first_seen_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    sealed_at INTEGER,
    seal_reason TEXT CHECK (seal_reason IN (
        'truncated', 'created_forward', 'head_changed', 'anchor_changed', 'stuck_force_reset'
    )),
    CHECK ((sealed_at IS NULL) = (seal_reason IS NULL))
) STRICT;

CREATE UNIQUE INDEX log_files_one_active_per_path ON log_files (path) WHERE sealed_at IS NULL;

CREATE TABLE log_events (
    log_file_id INTEGER NOT NULL REFERENCES log_files (id) ON DELETE RESTRICT,
    byte_offset INTEGER NOT NULL,
    kind TEXT CHECK (kind IN (
        'match_state_changed', 'game_state_update', 'deck_used', 'game_management_json',
        'match_deck_registered', 'tournament_state_changed', 'tournament_sync',
        'tournament_round_info', 'tournament_round_result', 'tournament_player_eliminated',
        'tournament_ended', 'draft_created', 'draft_state_changed', 'draft_selection',
        'draft_league_standing', 'draft_joined', 'draft_pod_state', 'draft_pack_opened',
        'draft_pending_pick', 'draft_ended', 'draft_pick_committed', 'league_pool_granted',
        'league_join_request', 'league_joined', 'league_dropped', 'login'
    )),
    match_token TEXT,
    match_id INTEGER,
    game_id INTEGER,
    tournament_token TEXT,
    league_token TEXT,
    league_event_id INTEGER,
    draft_token TEXT,
    logged_at INTEGER NOT NULL,
    raw BLOB NOT NULL,
    compressed INTEGER NOT NULL DEFAULT 0 CHECK (compressed IN (0, 1)),
    quarantined INTEGER NOT NULL DEFAULT 0 CHECK (quarantined IN (0, 1)),
    CHECK (kind IS NOT NULL OR quarantined = 1),
    PRIMARY KEY (log_file_id, byte_offset)
) STRICT;

CREATE INDEX log_events_match_token ON log_events (match_token) WHERE match_token IS NOT NULL;
CREATE INDEX log_events_match_id ON log_events (match_id) WHERE match_id IS NOT NULL;
CREATE INDEX log_events_game_id ON log_events (game_id) WHERE game_id IS NOT NULL;
CREATE INDEX log_events_tournament_token ON log_events (tournament_token) WHERE tournament_token IS NOT NULL;
CREATE INDEX log_events_draft_token ON log_events (draft_token) WHERE draft_token IS NOT NULL;
CREATE INDEX log_events_logins ON log_events (log_file_id, byte_offset) WHERE kind = 'login';
-- Matches that can ever be visible: their token has a join, a game management or
-- state change event in MatchJoinedEventUnderwayState (mtgo-match JOINED_STATE).
-- A compressed row cannot be searched, so it counts as one. Read at startup.
CREATE INDEX log_events_joins ON log_events (match_token)
    WHERE match_token IS NOT NULL
      AND kind IN ('game_management_json', 'match_state_changed')
      AND (compressed = 1 OR instr(raw, 'MatchJoinedEventUnderwayState') > 0);
-- League join and drop events are read on every leagues page: they are never
-- compacted and have no match token, so without this they cost a full scan.
CREATE INDEX log_events_leagues ON log_events (log_file_id, byte_offset)
    WHERE kind IN ('league_joined', 'league_dropped');
-- Draft events are read per account on every Limited page: they are never
-- compacted and the token-less ones have no subject, so without this they
-- cost a full scan.
CREATE INDEX log_events_drafts ON log_events (log_file_id, byte_offset)
    WHERE kind IN ('draft_created', 'draft_state_changed', 'draft_selection',
        'draft_league_standing', 'draft_joined', 'draft_pod_state', 'draft_pack_opened',
        'draft_pending_pick', 'draft_ended', 'draft_pick_committed', 'league_pool_granted');
-- Tournament events are queued for the API once each (07 spec section 8):
-- the enqueue reads the unqueued ones in (file, offset) order every run.
CREATE INDEX log_events_tournament_kinds ON log_events (log_file_id, byte_offset)
    WHERE kind IN ('tournament_sync', 'tournament_state_changed', 'tournament_round_result',
        'tournament_round_info', 'tournament_player_eliminated', 'tournament_ended');
-- The Limited pages check each limited match's registered deck for changes
-- (newest line, count) before reusing the parsed deck.
CREATE INDEX log_events_registered ON log_events (match_token, log_file_id, byte_offset)
    WHERE kind = 'match_deck_registered';
-- The overlays read a live game's newest (and first) state frame on every
-- frame (overlay spec 6 and 14): one seek instead of a walk of the game's
-- events and a sort.
CREATE INDEX log_events_frames ON log_events (game_id, log_file_id, byte_offset)
    WHERE kind = 'game_state_update';

CREATE TABLE match_event_archives (
    match_token TEXT PRIMARY KEY,
    raw BLOB NOT NULL,
    event_count INTEGER NOT NULL,
    compacted_at INTEGER NOT NULL
) STRICT;
