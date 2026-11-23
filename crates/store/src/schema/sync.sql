
-- API sync (07 spec section 6).

-- Every shipment to the API (0.x card_stat_ship_queue, tournament_observation_queue,
-- matches.submitted_at). payload is zstd JSON, emptied once sent or skipped.
CREATE TABLE outbox (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('match_report', 'tournament_observation', 'card_stats')),
    subject TEXT NOT NULL,
    payload BLOB NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'sending', 'sent', 'failed', 'skipped')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER,
    claimed_at INTEGER,
    last_error TEXT,
    sent_at INTEGER,
    created_at INTEGER NOT NULL,
    CHECK ((status = 'sending') = (claimed_at IS NOT NULL)),
    CHECK ((status = 'sent') = (sent_at IS NOT NULL)),
    UNIQUE (kind, subject)
) STRICT;
CREATE INDEX outbox_due ON outbox (kind, created_at) WHERE status = 'pending';

-- Cross-device sync, one row per synced row (0.x synced_hash, synced_at on
-- matches, decks, leagues). hash is what the server holds. pulled_key is the
-- touch key a manual match or manual league had when it arrived from the
-- server (pulled, or synced by 0.x): while its touch key still equals it, the
-- row is the server's copy, never pushed, and a re-pull replaces it.
CREATE TABLE synced_rows (
    type TEXT NOT NULL CHECK (type IN ('deck', 'league', 'match')),
    client_id TEXT NOT NULL,
    hash TEXT,
    synced_at INTEGER,
    built_hash TEXT,
    touch_key TEXT,
    changed_at INTEGER,
    carry TEXT CHECK (carry IS NULL OR json_valid(carry)),
    pulled_key TEXT,
    -- How many runs asked for the carry and were not given it (the server
    -- withholds it for a while) and when the first unanswered ask was; the
    -- row settles as an empty carry after a bounded number of runs and a day.
    carry_asks INTEGER NOT NULL DEFAULT 0,
    carry_first_ask_at INTEGER,
    PRIMARY KEY (type, client_id)
) STRICT;

CREATE TABLE sync_state (
    type TEXT PRIMARY KEY CHECK (type IN ('deck', 'league', 'match')),
    last_synced_at INTEGER,
    canonical_version INTEGER NOT NULL,
    full_cursor TEXT
) STRICT;

CREATE TABLE sync_rejections (
    type TEXT NOT NULL CHECK (type IN ('deck', 'league', 'match')),
    client_id TEXT NOT NULL,
    reason TEXT NOT NULL,
    hash TEXT NOT NULL,
    PRIMARY KEY (type, client_id)
) STRICT;
