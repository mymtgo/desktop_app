-- Matches and games (target schema section 6).

-- A match: derived by the projection from the log (`source = 'log'`),
-- imported from 0.x (`imported`), imported from MTGO's own match history
-- (`history`, 0.x "Import Match History"; "Delete all imported" removes only
-- these), or entered by hand (`manual`, which the projection never derives,
-- rebuilds, replaces or removes; it has no MTGO match id). `league_token`
-- and `game_structure` come from the match's join block (`League Token`,
-- `GameStructureCd`); 0.x imported rows leave them NULL.
-- `games_won_imported` and `games_lost_imported` are the game counts of a
-- match imported from MTGO's match history without a game log (deck stats
-- fold them in for a match without games, as 0.x did); NULL on every other
-- match.
--
-- The projection upserts a row in place by token and writes only its own
-- columns. `deck_version_id` is the deck version played, written by the deck
-- linker, an import or a manual match. `notes` (at most 5000 characters) and
-- `hidden_at` ("Remove from stats") are input: no derivation writes them, so
-- they survive every rebuild. A live match that replaces a 0.x or history
-- copy takes the copy's link and input where it has none. `detached_at` is
-- the projection's own: a log match that stops being visible while it carries
-- input (a note, a hidden mark, a hand league link, an `imported` limited
-- deck snapshot or a hand-set archetype) is kept, detached, instead of
-- deleted (its games go), and the projection clears it when it derives the
-- match again.
-- `league_id` and `tournament_id` are the league sync's (target schema section
-- 8): the run and the tournament a match is in, and so is
-- `registered_deck_checked_at`: the `compacted_at` of the match's archive
-- when the sync read it and found no registered deck line, so a compacted
-- match without one is not decoded again until its archive changes.
-- `tournament_token` is the league sync's too (07): the token of the newest
-- `tournament_round_info` event naming a log tournament match, written once
-- (0.x `LinkMatchToTournament`); the 0.x import copies 0.x's.
-- `archetypes_detected_at` is detection's own (archetypes spec section 4,
-- replacing 0.x's job flag): set when detection last ran for the match (or
-- the player chose no opponent archetype); NULL means "detect". No
-- derivation writes it. `games_synced_at` is the game sync's own: set the
-- first time it syncs a complete log match, so detection never reads a
-- half-written match (archetypes spec section 6.2).
-- `opponent_archetype_none` is input: the player chose no opponent
-- archetype (archetypes spec ruling 12). Set with the opponent row deleted,
-- cleared by any hand-set opponent; while it is set detection, a 0.x import
-- and a sync pull never write an opponent row, whatever the marker says.
-- `league_linked` is input: a
-- hand link to a manual league or a 0.x run's membership, whose `league_id`
-- the sync never writes. `derived_version` is the projection version that
-- derived a log row; on a manual row (never projected) it is the `revision`
-- the 0.x import last wrote, so `revision > derived_version` marks a manual
-- match the player edited in v1 since (the import's rerun then leaves it).
-- `replay_share_uuid`, `replay_share_url` and `replay_shared_at` are the
-- match's shared replay link on mymtgo.com (replays spec section 9): input,
-- all three or none, written only by the share and switch off edits and the
-- 0.x import, outside every sync touch key and the match bundle, so a
-- rebuild keeps them and sharing never makes a match dirty.
-- `legacy_history` is the 0.x import's (issue #313): 0.x's own
-- `matches.imported`, 1 for a match 0.x imported from MTGO's match history
-- (its deck lists hold only the cards seen, so the imported postboard rule
-- applies), 0 for one 0.x tracked from the log (also a 0.x older than the
-- flag, which predates 0.x's history import). A pulled match takes the
-- bundle's `imported`, so a pulled history match (`source = 'history'`)
-- holds 1 too. Every other row holds 0 (a live takeover resets it), so 1
-- is only ever on an `imported` or `history` row.
CREATE TABLE matches (
    id INTEGER PRIMARY KEY,
    token TEXT NOT NULL UNIQUE,
    mtgo_match_id INTEGER,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE RESTRICT,
    opponent_id INTEGER REFERENCES opponents (id) ON DELETE RESTRICT,
    source TEXT NOT NULL CHECK (source IN ('log', 'imported', 'manual', 'history')),
    format TEXT,
    match_type TEXT,
    tournament_event_id INTEGER,
    tournament_round INTEGER,
    state TEXT NOT NULL CHECK (state IN ('pending', 'in_progress', 'ended', 'complete', 'abandoned')),
    outcome TEXT NOT NULL CHECK (outcome IN ('win', 'loss', 'draw', 'unknown')),
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    last_activity_at INTEGER NOT NULL,
    needs_attention INTEGER NOT NULL DEFAULT 0 CHECK (needs_attention IN (0, 1)),
    attention_reason TEXT,
    revision INTEGER NOT NULL,
    derived_version INTEGER NOT NULL,
    league_token TEXT,
    game_structure TEXT,
    games_won_imported INTEGER,
    games_lost_imported INTEGER,
    deck_version_id INTEGER REFERENCES deck_versions (id) ON DELETE SET NULL,
    league_id INTEGER REFERENCES leagues (id) ON DELETE SET NULL,
    tournament_id INTEGER REFERENCES tournaments (id) ON DELETE SET NULL,
    tournament_token TEXT,
    registered_deck_checked_at INTEGER,
    detached_at INTEGER,
    archetypes_detected_at INTEGER,
    games_synced_at INTEGER,
    opponent_archetype_none INTEGER NOT NULL DEFAULT 0
        CHECK (opponent_archetype_none IN (0, 1)),
    league_linked INTEGER NOT NULL DEFAULT 0
        CHECK (league_linked IN (0, 1) AND (league_linked = 0 OR league_id IS NOT NULL)),
    notes TEXT CHECK (notes IS NULL OR length(notes) BETWEEN 1 AND 5000),
    hidden_at INTEGER,
    replay_share_uuid TEXT,
    replay_share_url TEXT,
    replay_shared_at INTEGER,
    legacy_history INTEGER NOT NULL DEFAULT 0 CHECK (legacy_history IN (0, 1)),
    CHECK (legacy_history = 0 OR source IN ('imported', 'history')),
    CHECK (source <> 'manual' OR mtgo_match_id IS NULL),
    CHECK ((replay_share_uuid IS NULL) = (replay_share_url IS NULL)
        AND (replay_share_uuid IS NULL) = (replay_shared_at IS NULL))
) STRICT;

CREATE INDEX matches_started_at ON matches (started_at);
CREATE INDEX matches_mtgo_match_id ON matches (mtgo_match_id) WHERE mtgo_match_id IS NOT NULL;
CREATE INDEX matches_account_league ON matches (account_id, league_token)
    WHERE league_token IS NOT NULL;
CREATE INDEX matches_account_tournament ON matches (account_id, tournament_event_id)
    WHERE tournament_event_id IS NOT NULL;
CREATE INDEX matches_deck_version_id ON matches (deck_version_id)
    WHERE deck_version_id IS NOT NULL;
-- The Opponents page's per-opponent reads, and the RESTRICT check on deleting an opponent.
CREATE INDEX matches_opponent_id ON matches (opponent_id) WHERE opponent_id IS NOT NULL;
CREATE INDEX matches_league_id ON matches (league_id) WHERE league_id IS NOT NULL;
CREATE INDEX matches_tournament_id ON matches (tournament_id) WHERE tournament_id IS NOT NULL;
-- The archetypes worker's queue (archetypes spec section 6.3), newest first.
CREATE INDEX matches_detect_due ON matches (started_at)
    WHERE archetypes_detected_at IS NULL AND state = 'complete';

-- A match's games are derived from it and go with it (CASCADE). The
-- projection upserts each game in place by `(match_id, game_number)` and
-- deletes only a game number the match no longer has. Opening hand: the cards
-- drawn for the hand that was kept (normally 7). Starting hand: what the
-- player started with after bottoming (London). The four hand sizes are
-- written with the game's `game_cards`, never by the projection.
CREATE TABLE games (
    id INTEGER PRIMARY KEY,
    match_id INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    mtgo_game_id INTEGER,
    game_number INTEGER NOT NULL,
    result TEXT NOT NULL CHECK (result IN ('win', 'loss', 'draw', 'unknown')),
    end_reason TEXT NOT NULL CHECK (end_reason IN ('win', 'concede', 'forfeit', 'disconnect', 'unknown')),
    on_play INTEGER CHECK (on_play IN (0, 1)),
    local_mulligans INTEGER,
    opp_mulligans INTEGER,
    local_opening_hand_size INTEGER,
    local_starting_hand_size INTEGER,
    opp_opening_hand_size INTEGER,
    opp_starting_hand_size INTEGER,
    local_die_roll INTEGER,
    opp_die_roll INTEGER,
    turn_count INTEGER,
    started_at INTEGER,
    ended_at INTEGER,
    UNIQUE (match_id, game_number),
    UNIQUE (match_id, mtgo_game_id)
) STRICT;

CREATE INDEX games_mtgo_game_id ON games (mtgo_game_id) WHERE mtgo_game_id IS NOT NULL;

-- One card in one game for one side: 0.x `card_game_stats` and the per-game card
-- lists of `game_player.opening_hand_json` and `deck_json` in one row (no JSON).
-- Terms: the opening hand is the hand drawn and kept (before bottoming, normally 7
-- cards); bottomed cards go under the library (London mulligan); the starting hand
-- is the opening hand minus the bottomed cards, what the player started with.
-- Written by the tracker's game sync for a log match, the player's edits for a
-- manual one, and the imports for the others; the hand sizes on `games` with them.
CREATE TABLE game_cards (
    game_id INTEGER NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    side TEXT NOT NULL CHECK (side IN ('local', 'opponent')),
    catalog_id INTEGER NOT NULL,
    -- The deck as played that game (local side; 0.x `quantity` is `main_qty`).
    main_qty INTEGER NOT NULL DEFAULT 0,
    side_qty INTEGER NOT NULL DEFAULT 0,
    -- Copies changed against game 1 (0.x flags, here counts).
    sided_in INTEGER NOT NULL DEFAULT 0,
    sided_out INTEGER NOT NULL DEFAULT 0,
    -- The hand (local side).
    opening_hand INTEGER NOT NULL DEFAULT 0,
    bottomed INTEGER NOT NULL DEFAULT 0,
    starting_hand INTEGER NOT NULL DEFAULT 0,
    -- The turn 1 draw on the draw, which 0.x showed beside the hand.
    first_draw INTEGER NOT NULL DEFAULT 0,
    -- Copies seen in visible zones during the game (0.x `seen`); on the
    -- opponent's side, every card the opponent revealed (0.x opponent `deck_json`).
    seen INTEGER NOT NULL DEFAULT 0,
    -- Shown or put onto the battlefield before the game (Chancellor, Leyline).
    pregame_revealed INTEGER NOT NULL DEFAULT 0,
    pregame_played INTEGER NOT NULL DEFAULT 0,
    -- Log counters (`cast`, `played`, `activated`, `cast_turn`): filled by the game sync
    -- from the game log, nullable where 0.x's NULL meant "no game log". The zone
    -- counters (`discarded`, `*_seen`, `has_zone_data`): the game sync's, from the
    -- frames (06); an imported game keeps 0.x's.
    cast INTEGER NOT NULL DEFAULT 0,
    played INTEGER,
    activated INTEGER,
    discarded INTEGER NOT NULL DEFAULT 0,
    cast_turn INTEGER,
    hand_seen INTEGER NOT NULL DEFAULT 0,
    graveyard_seen INTEGER NOT NULL DEFAULT 0,
    exile_seen INTEGER NOT NULL DEFAULT 0,
    battlefield_seen INTEGER NOT NULL DEFAULT 0,
    has_zone_data INTEGER NOT NULL DEFAULT 0 CHECK (has_zone_data IN (0, 1)),
    -- 1 on the local rows of an imported postboard game: the deck is the
    -- registered maindeck plus the sideboard cards the game log showed, so
    -- "sided in" counts only cards that were seen (issue #6 ruling).
    inferred INTEGER NOT NULL DEFAULT 0 CHECK (inferred IN (0, 1)),
    CHECK (starting_hand = opening_hand - bottomed),
    PRIMARY KEY (game_id, side, catalog_id)
) STRICT;
-- "Which cards do opponents play", "how often is X in my opening hand".
CREATE INDEX game_cards_catalog ON game_cards (catalog_id, side);

-- Casts by an alternative method (0.x's 23 nullable columns; 98% of 0.x rows had
-- none, foundation section 10). Filled by the game sync from the game log.
CREATE TABLE game_card_casts (
    game_id INTEGER NOT NULL,
    side TEXT NOT NULL,
    catalog_id INTEGER NOT NULL,
    method TEXT NOT NULL CHECK (method IN ('kicked', 'flashback', 'madness', 'evoked', 'warp',
        'free_cast', 'bargained', 'dashed', 'bestowed', 'replicated', 'spectacle', 'rebound',
        'escaped', 'ninjutsu', 'suspended', 'buyback', 'disturb', 'foretold', 'retraced',
        'mayhem', 'miracle', 'gifted', 'casualty')),
    count INTEGER NOT NULL CHECK (count >= 1),
    PRIMARY KEY (game_id, side, catalog_id, method),
    FOREIGN KEY (game_id, side, catalog_id)
        REFERENCES game_cards (game_id, side, catalog_id) ON DELETE CASCADE
) STRICT;

-- The hands the local player drew and sent back before the opening hand
-- (0.x `opening_hand_json.mulligans`), `hand` 1, 2, ... in order. Display only.
-- A mulligan the player could not remember has no rows.
CREATE TABLE game_mulligan_cards (
    game_id INTEGER NOT NULL REFERENCES games (id) ON DELETE CASCADE,
    hand INTEGER NOT NULL CHECK (hand >= 1),
    catalog_id INTEGER NOT NULL,
    quantity INTEGER NOT NULL CHECK (quantity >= 1),
    PRIMARY KEY (game_id, hand, catalog_id)
) STRICT;

-- Matches whose last projection failed. A match can fail before it ever had a
-- `matches` row (whose `account_id` needs the local player), so the failure
-- is kept here, by token. A later result for the match deletes it; a newer
-- projection version retries it.
CREATE TABLE match_failures (
    token TEXT PRIMARY KEY,
    reason TEXT NOT NULL,
    failed_at INTEGER NOT NULL,
    derived_version INTEGER NOT NULL
) STRICT;

-- Every read that lists or counts matches reads this view instead of
-- `matches`. SQLite folds it into the outer query, so the outer query still
-- walks the `matches` indexes. Hidden and detached matches are left out.
CREATE VIEW listed_matches AS
SELECT * FROM matches WHERE hidden_at IS NULL AND detached_at IS NULL;

-- 0.x `game_timelines`: one imported game's state frames (consecutive repeats
-- dropped, the last frame kept: `replay::drop_repeats`), zstd JSON. One
-- compressed row per game, not 0.x's row per frame (about 100 times smaller).
-- `times` is the zstd JSON list of each frame's 0.x timestamp text
-- (`HH:MM:SS` or `HH:MM:SS.mmm`; `replay::normalise_time`), or, for a match
-- pulled from another device, the bundle's ISO 8601 instant as sent
-- (`replay::normalise_time` keeps any other text as written), one per frame
-- in `raw`, same order. Live games have none: their frames are the stored events.
CREATE TABLE game_timelines (
    game_id INTEGER PRIMARY KEY REFERENCES games (id) ON DELETE CASCADE,
    raw BLOB NOT NULL,
    times BLOB NOT NULL,
    frame_count INTEGER NOT NULL
) STRICT;

-- 0.x `game_logs`: one match's decoded `.dat` game log, zstd JSON of its entries
-- (each with its own time). 0.x's `file_path`, `byte_offset`, `decoded_version`,
-- `first_timestamp` and `players` were its reader's bookkeeping. The tracker
-- stores a live match's once its file is found; the imports store theirs.
CREATE TABLE game_logs (
    match_id INTEGER PRIMARY KEY REFERENCES matches (id) ON DELETE CASCADE,
    decoded_entries BLOB NOT NULL,
    entry_count INTEGER NOT NULL,
    decoded_at INTEGER NOT NULL
) STRICT;
