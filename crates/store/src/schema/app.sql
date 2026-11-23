
-- Settings and app state (target schema section 14).

CREATE TABLE settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- The version of each derivation the stored rows were built with.
CREATE TABLE derivations (
    name TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    rebuilt_at INTEGER NOT NULL
) STRICT;
