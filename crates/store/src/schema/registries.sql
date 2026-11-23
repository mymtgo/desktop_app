-- Accounts and opponents (target schema section 5): registries, never deleted.

-- `tracked` is input (release spec 5): written only by `AccountEdit::SetTracked`
-- and the 0.x import. An untracked account's new log matches are not built.
-- `tracked_chosen` is input: 1 once the user set tracking in v1
-- (`AccountEdit::SetTracked`). The 0.x import carries 0.x's untracked flag
-- only while it is 0, so a v1 choice is never overwritten (issue #310).
CREATE TABLE accounts (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    login_id INTEGER UNIQUE,
    first_seen_at INTEGER NOT NULL,
    tracked INTEGER NOT NULL DEFAULT 1 CHECK (tracked IN (0, 1)),
    tracked_chosen INTEGER NOT NULL DEFAULT 0 CHECK (tracked_chosen IN (0, 1)),
    -- Derived: the log time of the newest Login line that named the account
    -- (the current account rule, release spec 5), kept so the pages read no
    -- log events.
    last_login_at INTEGER
) STRICT;

CREATE TABLE opponents (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    first_seen_at INTEGER NOT NULL
) STRICT;

-- Derived (release spec 5): the log matches of an untracked account the
-- tracker did not build. Their events stay stored; turning tracking back on
-- rebuilds each one and clears its row. A token is never both here and in
-- `matches`.
CREATE TABLE untracked_matches (
    match_token TEXT PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE RESTRICT,
    skipped_at INTEGER NOT NULL
) STRICT;

CREATE INDEX untracked_matches_account ON untracked_matches (account_id);
