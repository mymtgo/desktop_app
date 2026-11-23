//! The console's starter views (spec section 5.2). Each one is created as a
//! temp view on every console connection, so it never touches the database
//! file.

/// One console view: its name, its `SELECT` and its columns as
/// `(name, description)` pairs for the schema pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewDef {
    pub name: &'static str,
    pub sql: &'static str,
    pub columns: &'static [(&'static str, &'static str)],
}

/// Every console view, in the order the schema pane lists them.
pub const ALL: &[ViewDef] = &[
    MATCHES, GAMES, CARD_GAMES, DECKS, LEAGUES, CARDS, ARCHETYPES,
];

// Scope (spec 5.2): every account on every platform; only listed matches
// (`listed_matches`: not hidden, not detached) that have finished
// (`state = 'complete'`, the match list's rule). Times come twice: UTC unix
// milliseconds (`*_ms`) and ISO 8601 UTC text (`*_at`).

/// One row per finished listed match. Names are what the pages show
/// (`COALESCE(display_name, username)`); game counts fall back to the
/// history's counts for a match without game rows, as the deck stats do; a
/// match in a deleted or detached league is not in a league, as on every
/// page.
const MATCHES: ViewDef = ViewDef {
    name: "v_matches",
    sql: "
SELECT m.id AS match_id,
       a.platform AS platform,
       COALESCE(a.display_name, a.username) AS account,
       COALESCE(o.display_name, o.username) AS opponent,
       m.format AS format,
       m.match_type AS match_type,
       m.game_structure AS game_structure,
       m.outcome AS outcome,
       CASE WHEN EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)
            THEN (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'win')
            ELSE m.games_won_imported END AS games_won,
       CASE WHEN EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)
            THEN (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'loss')
            ELSE m.games_lost_imported END AS games_lost,
       strftime('%Y-%m-%dT%H:%M:%SZ', m.started_at / 1000, 'unixepoch') AS started_at,
       m.started_at AS started_ms,
       strftime('%Y-%m-%dT%H:%M:%SZ', m.ended_at / 1000, 'unixepoch') AS ended_at,
       (m.ended_at - m.started_at) / 1000 AS duration_s,
       d.name AS deck_name,
       mine.name AS archetype,
       theirs.name AS opponent_archetype,
       l.id AS league_id,
       m.source AS source
FROM listed_matches m
JOIN accounts a ON a.id = m.account_id
LEFT JOIN opponents o ON o.id = m.opponent_id
LEFT JOIN deck_versions v ON v.id = m.deck_version_id
LEFT JOIN decks d ON d.id = v.deck_id
LEFT JOIN match_archetypes my_seat ON my_seat.match_id = m.id AND my_seat.is_opponent = 0
LEFT JOIN archetypes mine ON mine.id = my_seat.archetype_id
LEFT JOIN match_archetypes their_seat
    ON their_seat.match_id = m.id AND their_seat.is_opponent = 1
LEFT JOIN archetypes theirs ON theirs.id = their_seat.archetype_id
LEFT JOIN listed_leagues l ON l.id = m.league_id AND l.detached_at IS NULL
WHERE m.state = 'complete'",
    columns: &[
        ("match_id", "The match's row id; joins v_games.match_id."),
        (
            "platform",
            "mtgo, arena or endstep: the platform of the match's account.",
        ),
        ("account", "Your account that played the match."),
        ("opponent", "The opponent's name."),
        ("format", "The format code, such as CMODERN."),
        (
            "match_type",
            "The event type the client gave, such as Modern or League.",
        ),
        (
            "game_structure",
            "MTGO's game structure text; bo1 or bo3 on Arena.",
        ),
        ("outcome", "win, loss, draw or unknown."),
        (
            "games_won",
            "Games you won (the history's count when the match has no games).",
        ),
        (
            "games_lost",
            "Games you lost (the history's count when the match has no games).",
        ),
        ("started_at", "When the match started, ISO 8601 UTC."),
        (
            "started_ms",
            "When the match started, UTC unix milliseconds.",
        ),
        (
            "ended_at",
            "When the match ended, ISO 8601 UTC; empty when unknown.",
        ),
        (
            "duration_s",
            "The match's length in seconds; empty when the end is unknown.",
        ),
        (
            "deck_name",
            "The name of the deck you played; empty when no deck is linked.",
        ),
        ("archetype", "Your deck's archetype in this match."),
        (
            "opponent_archetype",
            "The opponent's archetype in this match.",
        ),
        (
            "league_id",
            "The league or run the match is in; joins v_leagues.league_id.",
        ),
        (
            "source",
            "log (tracked), imported, manual (entered by hand) or history (MTGO match history).",
        ),
    ],
};

/// One row per game of a finished listed match.
const GAMES: ViewDef = ViewDef {
    name: "v_games",
    sql: "
SELECT g.match_id AS match_id,
       g.game_number AS game_number,
       a.platform AS platform,
       m.format AS format,
       g.result AS result,
       g.end_reason AS end_reason,
       g.on_play AS on_play,
       g.local_mulligans AS mulligans,
       g.opp_mulligans AS opp_mulligans,
       g.local_starting_hand_size AS starting_hand_size,
       g.turn_count AS turns,
       strftime('%Y-%m-%dT%H:%M:%SZ', g.started_at / 1000, 'unixepoch') AS started_at,
       g.started_at AS started_ms,
       (g.ended_at - g.started_at) / 1000 AS duration_s,
       theirs.name AS opponent_archetype
FROM games g
JOIN listed_matches m ON m.id = g.match_id
JOIN accounts a ON a.id = m.account_id
LEFT JOIN match_archetypes their_seat
    ON their_seat.match_id = m.id AND their_seat.is_opponent = 1
LEFT JOIN archetypes theirs ON theirs.id = their_seat.archetype_id
WHERE m.state = 'complete'",
    columns: &[
        ("match_id", "The game's match; joins v_matches.match_id."),
        (
            "game_number",
            "1 for the first game of the match, then 2, 3.",
        ),
        ("platform", "mtgo, arena or endstep."),
        ("format", "The match's format code, such as CMODERN."),
        ("result", "win, loss, draw or unknown."),
        (
            "end_reason",
            "win, concede, forfeit, disconnect or unknown.",
        ),
        (
            "on_play",
            "1 when you were on the play, 0 on the draw; empty when unknown.",
        ),
        ("mulligans", "How many times you mulliganed."),
        ("opp_mulligans", "How many times the opponent mulliganed."),
        (
            "starting_hand_size",
            "Cards in your hand after bottoming, when known.",
        ),
        ("turns", "The game's turn count, when known."),
        ("started_at", "When the game started, ISO 8601 UTC."),
        (
            "started_ms",
            "When the game started, UTC unix milliseconds.",
        ),
        ("duration_s", "The game's length in seconds, when known."),
        (
            "opponent_archetype",
            "The opponent's archetype in this match.",
        ),
    ],
};

/// One row per card per seat per game: the Card Stats tab's grain and its
/// raw counts (`card_stats::card_stats`). The games are the tab's: those of
/// finished listed matches, through the deck version they link to. The rows
/// are the tab's too (`card_stats::deck_row` or `card_stats::other_row`,
/// inlined below; a test keeps the text the same): on your side a maindeck
/// card, a postboard sideboard or sided card, or a variant or face printing
/// of a card; every opponent row. Printings of one oracle id add up into one
/// row, keyed by the oracle id or `#{catalog id}` for a card without one,
/// and named by the highest catalog id among them ("Card {id}" without card
/// data).
///
/// Faces fold as the tab folds them (`card_stats::FaceNames`): a row whose
/// card is a stub with no oracle id, named by the log, takes the oracle id
/// of the first card (by catalog id) with that name as one of its ` // `
/// faces, case-sensitive; the tab trims spaces around each ` // ` face; the
/// view matches the name as stored. On your side that applies only to rows that are
/// not deck rows, and such a row (a face, or a variant printing) is kept
/// only when its card has copies in your deck that game; any other stub
/// (a token, a copy, a card before its data arrived) is left out. Every
/// opponent row is kept, with its face folded.
///
/// Unlike the tab, nothing is capped: a variant's or face's copies add to
/// `seen` rather than taking the larger count. An opponent's card has rows
/// only in the games it was shown, where the tab counts every game. The
/// cast count is `casts`, since `cast` is an SQL keyword a query could not
/// use unquoted.
const CARD_GAMES: ViewDef = ViewDef {
    name: "v_card_games",
    sql: "
WITH card_rows AS (
    SELECT m.id AS match_id,
           g.id AS game_id,
           g.game_number AS game_number,
           g.on_play AS on_play,
           (g.result = 'win') AS won,
           a.platform AS platform,
           m.format AS format,
           d.id AS deck_id,
           d.name AS deck_name,
           v.id AS deck_version_id,
           mine.name AS archetype,
           theirs.name AS opponent_archetype,
           gc.side AS seat,
           gc.catalog_id AS catalog_id,
           c.oracle_id AS oracle_id,
           c.name AS catalog_name,
           gc.main_qty AS main_qty,
           gc.sided_in AS sided_in,
           gc.sided_out AS sided_out,
           gc.opening_hand AS opening_hand,
           gc.starting_hand AS starting_hand,
           gc.seen AS seen,
           gc.cast AS cast_count,
           (gc.side <> 'local' OR gc.main_qty > 0
     OR (g.game_number > 1 AND (gc.side_qty > 0 OR gc.sided_in > 0 OR gc.sided_out > 0
         OR (m.source <> 'log' AND gc.seen = 0 AND gc.cast = 0 AND COALESCE(gc.played, 0) = 0))))
               AS deck_row,
           (gc.side = 'local' AND gc.main_qty = 0 AND EXISTS (
     SELECT 1 FROM cards va
     WHERE va.catalog_id = gc.catalog_id
       AND ((va.oracle_id IS NULL AND va.name <> '')
         OR (va.oracle_id IS NOT NULL AND EXISTS (
             SELECT 1 FROM cards vb
             JOIN game_cards vd ON vd.game_id = gc.game_id AND vd.side = 'local'
                               AND vd.catalog_id = vb.catalog_id
             WHERE vb.oracle_id = va.oracle_id AND vd.main_qty > 0)))))
               AS other_row
    FROM deck_versions v
    JOIN decks d ON d.id = v.deck_id
    JOIN listed_matches m ON m.deck_version_id = v.id
    JOIN accounts a ON a.id = m.account_id
    JOIN games g ON g.match_id = m.id
    JOIN game_cards gc ON gc.game_id = g.id
    LEFT JOIN cards c ON c.catalog_id = gc.catalog_id
    LEFT JOIN match_archetypes my_seat ON my_seat.match_id = m.id AND my_seat.is_opponent = 0
    LEFT JOIN archetypes mine ON mine.id = my_seat.archetype_id
    LEFT JOIN match_archetypes their_seat
        ON their_seat.match_id = m.id AND their_seat.is_opponent = 1
    LEFT JOIN archetypes theirs ON theirs.id = their_seat.archetype_id
    WHERE m.state = 'complete'
),
faced AS (
    SELECT r.*,
           CASE WHEN r.oracle_id IS NULL AND r.catalog_name <> ''
                     AND instr(r.catalog_name, ' // ') = 0
                     AND (r.seat <> 'local' OR NOT r.deck_row)
                THEN (SELECT f.catalog_id FROM cards f
                      WHERE f.oracle_id IS NOT NULL AND f.oracle_id <> '' AND f.name IS NOT NULL
                        AND instr(' // ' || f.name || ' // ', ' // ' || r.catalog_name || ' // ') > 0
                      ORDER BY f.catalog_id LIMIT 1)
           END AS face_id
    FROM card_rows r
    WHERE r.deck_row OR r.other_row
),
keyed AS (
    SELECT fr.*,
           COALESCE(face.oracle_id, fr.oracle_id, '#' || fr.catalog_id) AS card_key,
           CASE WHEN fr.seat = 'local' AND NOT fr.deck_row THEN NULL
                ELSE COALESCE(fr.face_id, fr.catalog_id) END AS shown_catalog_id
    FROM faced fr
    LEFT JOIN cards face ON face.catalog_id = fr.face_id
    WHERE fr.deck_row OR fr.oracle_id IS NOT NULL
       OR (face.oracle_id IS NOT NULL AND EXISTS (
           SELECT 1 FROM game_cards d
           JOIN cards dc ON dc.catalog_id = d.catalog_id
           WHERE d.game_id = fr.game_id AND d.side = 'local' AND d.main_qty > 0
             AND dc.oracle_id = face.oracle_id))
),
grouped AS (
    SELECT k.match_id AS match_id,
           k.game_id AS game_id,
           k.game_number AS game_number,
           k.on_play AS on_play,
           k.won AS won,
           k.platform AS platform,
           k.format AS format,
           k.deck_id AS deck_id,
           k.deck_name AS deck_name,
           k.deck_version_id AS deck_version_id,
           k.archetype AS archetype,
           k.opponent_archetype AS opponent_archetype,
           k.seat AS seat,
           k.card_key AS card_key,
           MAX(k.shown_catalog_id) AS shown_id,
           SUM(k.main_qty) AS in_deck,
           SUM(k.sided_in) AS sided_in,
           SUM(k.sided_out) AS sided_out,
           SUM(k.opening_hand) AS in_opening_hand,
           SUM(k.starting_hand) AS in_starting_hand,
           SUM(k.seen) AS seen,
           SUM(k.cast_count) AS casts
    FROM keyed k
    GROUP BY k.game_id, k.seat, k.card_key
)
SELECT r.match_id AS match_id,
       r.game_id AS game_id,
       r.game_number AS game_number,
       (r.game_number > 1) AS postboard,
       r.on_play AS on_play,
       r.won AS won,
       r.platform AS platform,
       r.format AS format,
       r.deck_id AS deck_id,
       r.deck_name AS deck_name,
       r.deck_version_id AS deck_version_id,
       r.archetype AS archetype,
       r.opponent_archetype AS opponent_archetype,
       CASE r.seat WHEN 'local' THEN 'me' ELSE 'opponent' END AS side,
       COALESCE(shown.name, 'Card ' || r.shown_id) AS card_name,
       r.card_key AS card_key,
       shown.type AS card_type,
       r.in_deck AS in_deck,
       r.sided_in AS sided_in,
       r.sided_out AS sided_out,
       r.in_opening_hand AS in_opening_hand,
       r.in_starting_hand AS in_starting_hand,
       r.seen AS seen,
       r.casts AS casts
FROM grouped r
LEFT JOIN cards shown ON shown.catalog_id = r.shown_id",
    columns: &[
        ("match_id", "The game's match; joins v_matches.match_id."),
        ("game_id", "The game's row id."),
        (
            "game_number",
            "1 for the first game of the match, then 2, 3.",
        ),
        ("postboard", "1 for game 2 or later, else 0."),
        (
            "on_play",
            "1 when you were on the play, 0 on the draw; empty when unknown.",
        ),
        (
            "won",
            "1 when you won the game, else 0 (a draw is not won, as on the Card Stats tab).",
        ),
        ("platform", "mtgo, arena or endstep."),
        ("format", "The match's format code, such as CMODERN."),
        ("deck_id", "The deck you played; joins v_decks.deck_id."),
        ("deck_name", "The deck's name."),
        ("deck_version_id", "The version of the deck the match used."),
        ("archetype", "Your deck's archetype in this match."),
        (
            "opponent_archetype",
            "The opponent's archetype in this match.",
        ),
        (
            "side",
            "me for your cards, opponent for the cards the opponent showed. An opponent's card has rows only in the games it was shown (so a count of rows is not the Card Stats tab's opponent games), and its in_deck and hand columns are 0.",
        ),
        (
            "card_name",
            "The card's name; Card {id} when the card has no card data.",
        ),
        (
            "card_key",
            "The card's oracle id (all printings together), or #{catalog id} without card data.",
        ),
        ("card_type", "The card's type line."),
        ("in_deck", "Copies in your main deck that game."),
        ("sided_in", "Copies sided in against game 1."),
        ("sided_out", "Copies sided out against game 1."),
        (
            "in_opening_hand",
            "Copies in your opening hand (as drawn, before bottoming).",
        ),
        (
            "in_starting_hand",
            "Copies in your starting hand (after bottoming); the tab's kept copies.",
        ),
        ("seen", "Copies seen during the game."),
        ("casts", "Times the card was cast."),
    ],
};

/// One row per deck, with its finished listed matches. A deck's platform is
/// its account's, else the platform it was pulled for, else MTGO
/// (`platform::deck_platform`).
const DECKS: ViewDef = ViewDef {
    name: "v_decks",
    sql: "
SELECT d.id AS deck_id,
       d.name AS name,
       d.format AS format,
       COALESCE(a.platform, d.pulled_platform, 'mtgo') AS platform,
       d.color_identity AS colors,
       ar.name AS archetype,
       (d.archived_at IS NOT NULL) AS archived,
       COUNT(m.id) AS matches,
       COALESCE(SUM(m.outcome = 'win'), 0) AS wins,
       COALESCE(SUM(m.outcome = 'loss'), 0) AS losses
FROM decks d
LEFT JOIN accounts a ON a.id = d.account_id
LEFT JOIN archetypes ar ON ar.id = d.archetype_id
LEFT JOIN deck_versions v ON v.deck_id = d.id
LEFT JOIN listed_matches m ON m.deck_version_id = v.id AND m.state = 'complete'
GROUP BY d.id",
    columns: &[
        ("deck_id", "The deck's row id."),
        ("name", "The deck's name."),
        ("format", "The deck's format code, such as CMODERN."),
        ("platform", "mtgo, arena or endstep."),
        ("colors", "The deck's colour identity, such as UR."),
        ("archetype", "The archetype set on the deck."),
        ("archived", "1 when the deck is archived, else 0."),
        (
            "matches",
            "Finished matches played with any version of the deck.",
        ),
        ("wins", "Those matches won."),
        ("losses", "Those matches lost."),
    ],
};

/// One row per league or run the player has not deleted and the league sync
/// has not detached (`stored_runs`). The record is the finished listed
/// matches won and lost, each raised to the course's stated count when that
/// is higher and the course states both counts (an Arena course run,
/// `leagues::shown_record`).
const LEAGUES: ViewDef = ViewDef {
    name: "v_leagues",
    sql: "
SELECT l.id AS league_id,
       l.name AS name,
       l.kind AS kind,
       l.format AS format,
       COALESCE(l.user_state, l.state) AS state,
       CASE WHEN l.course_wins IS NOT NULL AND l.course_losses IS NOT NULL
            THEN MAX(l.course_wins, l.tracked_wins) ELSE l.tracked_wins END AS wins,
       CASE WHEN l.course_wins IS NOT NULL AND l.course_losses IS NOT NULL
            THEN MAX(l.course_losses, l.tracked_losses) ELSE l.tracked_losses END AS losses,
       strftime('%Y-%m-%dT%H:%M:%SZ', l.started_at / 1000, 'unixepoch') AS started_at,
       l.started_at AS started_ms,
       d.name AS deck_name
FROM (
    SELECT listed.*,
           (SELECT COUNT(*) FROM listed_matches m
            WHERE m.league_id = listed.id AND m.state = 'complete'
              AND m.outcome = 'win') AS tracked_wins,
           (SELECT COUNT(*) FROM listed_matches m
            WHERE m.league_id = listed.id AND m.state = 'complete'
              AND m.outcome = 'loss') AS tracked_losses
    FROM listed_leagues listed
    WHERE listed.detached_at IS NULL
) l
LEFT JOIN decks d ON d.id = l.deck_id",
    columns: &[
        (
            "league_id",
            "The league's row id; joins v_matches.league_id.",
        ),
        ("name", "The league's name."),
        ("kind", "constructed, draft or sealed."),
        ("format", "The format code, when known."),
        (
            "state",
            "active, complete, partial or dropped, as the league's card shows.",
        ),
        ("wins", "Matches won in the league."),
        ("losses", "Matches lost in the league."),
        ("started_at", "When the league started, ISO 8601 UTC."),
        (
            "started_ms",
            "When the league started, UTC unix milliseconds.",
        ),
        ("deck_name", "The league's deck, when it has one."),
    ],
};

/// The card catalog, one row per printing.
const CARDS: ViewDef = ViewDef {
    name: "v_cards",
    sql: "
SELECT c.catalog_id AS catalog_id,
       c.name AS name,
       c.type AS type,
       c.mana_cost AS mana_cost,
       c.cmc AS cmc,
       c.colors AS colors,
       c.rarity AS rarity,
       c.set_code AS set_code
FROM cards c",
    columns: &[
        ("catalog_id", "The printing's catalog id."),
        ("name", "The card's name; empty until card data is fetched."),
        ("type", "The card's type line."),
        ("mana_cost", "The mana cost, such as {1}{U}{R}."),
        ("cmc", "The mana value."),
        ("colors", "The card's colours."),
        ("rarity", "The printing's rarity."),
        ("set_code", "The printing's set code."),
    ],
};

/// One row per archetype in use (not retired, not merged away), with the
/// finished listed matches of each seat.
const ARCHETYPES: ViewDef = ViewDef {
    name: "v_archetypes",
    sql: "
SELECT ar.id AS archetype_id,
       ar.name AS name,
       ar.format AS format,
       ar.color_identity AS colors,
       ar.source AS source,
       (SELECT COUNT(*) FROM match_archetypes ma
        JOIN listed_matches m ON m.id = ma.match_id
        WHERE ma.archetype_id = ar.id AND ma.is_opponent = 0
          AND m.state = 'complete') AS matches_as_me,
       (SELECT COUNT(*) FROM match_archetypes ma
        JOIN listed_matches m ON m.id = ma.match_id
        WHERE ma.archetype_id = ar.id AND ma.is_opponent = 1
          AND m.state = 'complete') AS matches_against
FROM archetypes ar
WHERE ar.retired_at IS NULL AND ar.merged_into_id IS NULL",
    columns: &[
        ("archetype_id", "The archetype's row id."),
        ("name", "The archetype's name."),
        (
            "format",
            "The format key; empty for Homebrew and Rogue, which fit every format.",
        ),
        ("colors", "The archetype's colour identity."),
        ("source", "downloaded, manual, fallback or commander."),
        ("matches_as_me", "Finished matches where you played it."),
        (
            "matches_against",
            "Finished matches where an opponent played it.",
        ),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::console::open_console;
    use crate::test_support::{
        OTHER, TOKEN, db, hide, insert_manual_league, link_league_match, match_id,
        move_match_to_arena, seed_endstep, view,
    };
    use crate::write::{MatchWrite, WriteBatch, commit};
    use fixture_tools::TestDir;
    use mtgo_match::{MatchState, MatchView, Outcome};
    use rusqlite::Connection;

    const HIDDEN: &str = "q-hidden";
    const LIVE: &str = "q-live";
    const ENDSTEP: &str = "q-endstep";
    const STARTED_MS: i64 = 1_787_000_000_000;

    /// The row ids of the seeded matches.
    struct Seeded {
        mtgo: i64,
        arena: i64,
        endstep: i64,
        hidden: i64,
        live: i64,
        endstep_deck: i64,
    }

    fn store_views(conn: &mut Connection, views: Vec<MatchView>) {
        let matches = views
            .into_iter()
            .map(|view| MatchWrite::Upsert {
                view: Box::new(view),
                revision: 1,
            })
            .collect();
        commit(
            conn,
            &WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        )
        .expect("matches");
    }

    /// One finished MTGO match (Homebrew against Rogue), one Arena match with
    /// no games and the history's counts, one Endstep match with a deck, one
    /// hidden and one in-progress match, opened as a console connection.
    fn seeded(label: &str) -> (TestDir, Connection, Seeded) {
        let (dir, mut conn) = db(label);
        let mut live = view(LIVE, "Qopponent", STARTED_MS + 30_000);
        live.state = MatchState::InProgress;
        live.outcome = Outcome::Unknown;
        live.ended_at_ms = None;
        store_views(
            &mut conn,
            vec![
                view(TOKEN, "Qopponent", STARTED_MS),
                view(OTHER, "Qopponent", STARTED_MS + 10_000),
                view(HIDDEN, "Qopponent", STARTED_MS + 20_000),
                live,
            ],
        );
        move_match_to_arena(&conn, OTHER);
        hide(&conn, HIDDEN);
        let endstep = seed_endstep(&mut conn, ENDSTEP);
        // League 1 holds the Endstep win and the hidden match, and states
        // only a course win count (so it is not applied); league 2, which
        // holds the MTGO match, is deleted; league 3, which holds the Arena
        // match, is detached; league 4 states its course record whole.
        let deck = Some(endstep.deck_id);
        insert_manual_league(&conn, 1, None, deck, "Qleague", STARTED_MS, STARTED_MS);
        insert_manual_league(&conn, 2, None, deck, "Qremoved", STARTED_MS, STARTED_MS);
        insert_manual_league(&conn, 3, None, deck, "Qdetached", STARTED_MS, STARTED_MS);
        insert_manual_league(&conn, 4, None, None, "Qcourse", STARTED_MS, STARTED_MS);
        link_league_match(&conn, 1, ENDSTEP);
        link_league_match(&conn, 1, HIDDEN);
        link_league_match(&conn, 2, TOKEN);
        link_league_match(&conn, 3, OTHER);
        conn.execute_batch(
            "UPDATE leagues SET course_wins = 5 WHERE id = 1;
             UPDATE leagues SET removed_at = 1 WHERE id = 2;
             UPDATE leagues SET detached_at = 1 WHERE id = 3;
             UPDATE leagues SET course_wins = 3, course_losses = 2 WHERE id = 4;",
        )
        .expect("league states");
        // Decks without an account: one pulled for Arena, one from MTGO.
        conn.execute_batch(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source,
                 pulled_platform)
             VALUES ('q-pulled', 'Qpulled', 'CMODERN', 0, 0, 'imported', 'arena'),
                 ('q-file', 'Qfile', 'CMODERN', 0, 0, 'file', NULL);",
        )
        .expect("decks without an account");
        let seeded = Seeded {
            mtgo: match_id(&conn, TOKEN),
            arena: match_id(&conn, OTHER),
            endstep: match_id(&conn, ENDSTEP),
            hidden: match_id(&conn, HIDDEN),
            live: match_id(&conn, LIVE),
            endstep_deck: endstep.deck_id,
        };
        conn.execute_batch(&format!(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES ({mtgo}, 0, 1, 1.0, 1), ({mtgo}, 1, 2, 1.0, 1);
             DELETE FROM games WHERE match_id = {arena};
             UPDATE matches SET source = 'history', games_won_imported = 1,
                 games_lost_imported = 2 WHERE id = {arena};",
            mtgo = seeded.mtgo,
            arena = seeded.arena,
        ))
        .expect("archetypes and history counts");
        drop(conn);
        let console = open_console(&dir.join("mymtgo.sqlite")).expect("console");
        (dir, console, seeded)
    }

    fn text(conn: &Connection, sql: &str) -> Option<String> {
        conn.query_row(sql, [], |row| row.get(0)).expect(sql)
    }

    fn int(conn: &Connection, sql: &str) -> Option<i64> {
        conn.query_row(sql, [], |row| row.get(0)).expect(sql)
    }

    #[test]
    fn every_view_selects_on_a_migrated_database() {
        let (_dir, conn, _seeded) = seeded("console-views-select");
        let names: Vec<&str> = ALL.iter().map(|view| view.name).collect();
        assert_eq!(
            names,
            vec![
                "v_matches",
                "v_games",
                "v_card_games",
                "v_decks",
                "v_leagues",
                "v_cards",
                "v_archetypes"
            ]
        );
        for def in ALL {
            let sql = format!("SELECT * FROM {} LIMIT 1", def.name);
            let mut stmt = conn.prepare(&sql).expect(&sql);
            let columns: Vec<String> = stmt
                .column_names()
                .into_iter()
                .map(|name| name.to_string())
                .collect();
            let wanted: Vec<&str> = def.columns.iter().map(|(name, _)| *name).collect();
            assert_eq!(columns, wanted, "{}", def.name);
            let mut rows = stmt.query([]).expect(&sql);
            rows.next().expect(&sql);
            for (name, description) in def.columns {
                assert!(!description.is_empty(), "{}.{name}", def.name);
            }
        }
    }

    #[test]
    fn v_matches_lists_finished_listed_matches_on_every_platform() {
        let (_dir, conn, seeded) = seeded("console-views-matches");
        let mut stmt = conn
            .prepare("SELECT match_id FROM v_matches ORDER BY match_id")
            .expect("ids");
        let ids: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .expect("ids")
            .map(|id| id.expect("id"))
            .collect();
        let mut wanted = vec![seeded.mtgo, seeded.arena, seeded.endstep];
        wanted.sort_unstable();
        assert_eq!(ids, wanted);
        assert!(!ids.contains(&seeded.hidden));
        assert!(!ids.contains(&seeded.live));

        let row = |column: &str, id: i64| {
            text(
                &conn,
                &format!("SELECT {column} FROM v_matches WHERE match_id = {id}"),
            )
        };
        assert_eq!(row("platform", seeded.mtgo).as_deref(), Some("mtgo"));
        assert_eq!(row("platform", seeded.arena).as_deref(), Some("arena"));
        assert_eq!(row("platform", seeded.endstep).as_deref(), Some("endstep"));
        assert_eq!(row("account", seeded.mtgo).as_deref(), Some("Qlocal"));
        assert_eq!(row("account", seeded.endstep).as_deref(), Some("Player"));
        assert_eq!(row("opponent", seeded.mtgo).as_deref(), Some("Qopponent"));
        assert_eq!(row("opponent", seeded.endstep).as_deref(), Some("Opponent"));
        assert_eq!(row("archetype", seeded.mtgo).as_deref(), Some("Homebrew"));
        assert_eq!(
            row("opponent_archetype", seeded.mtgo).as_deref(),
            Some("Rogue")
        );
        assert_eq!(
            row("deck_name", seeded.endstep).as_deref(),
            Some("Qendstep deck")
        );

        let games = |id: i64| {
            (
                int(
                    &conn,
                    &format!("SELECT games_won FROM v_matches WHERE match_id = {id}"),
                ),
                int(
                    &conn,
                    &format!("SELECT games_lost FROM v_matches WHERE match_id = {id}"),
                ),
            )
        };
        assert_eq!(games(seeded.mtgo), (Some(2), Some(1)));
        // No games: the history's counts.
        assert_eq!(games(seeded.arena), (Some(1), Some(2)));

        let started_at = chrono::DateTime::from_timestamp_millis(STARTED_MS)
            .expect("time")
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        assert_eq!(row("started_at", seeded.mtgo), Some(started_at));
        assert_eq!(
            int(
                &conn,
                &format!(
                    "SELECT started_ms FROM v_matches WHERE match_id = {}",
                    seeded.mtgo
                )
            ),
            Some(STARTED_MS)
        );
        assert_eq!(
            int(
                &conn,
                &format!(
                    "SELECT duration_s FROM v_matches WHERE match_id = {}",
                    seeded.mtgo
                )
            ),
            Some(4)
        );
    }

    #[test]
    fn v_games_has_the_games_of_listed_finished_matches() {
        let (_dir, conn, seeded) = seeded("console-views-games");
        // The MTGO and Endstep matches have three games each; the Arena
        // match has none, the hidden and live ones are left out.
        assert_eq!(int(&conn, "SELECT COUNT(*) FROM v_games"), Some(6));
        assert_eq!(
            text(
                &conn,
                &format!(
                    "SELECT result || ',' || on_play || ',' || opp_mulligans || ',' || turns
                     || ',' || opponent_archetype || ',' || started_at || ',' || started_ms
                     FROM v_games
                     WHERE match_id = {} AND game_number = 2",
                    seeded.mtgo
                )
            )
            .as_deref(),
            Some("loss,0,1,9,Rogue,1970-01-01T00:00:01Z,1000")
        );
    }

    #[test]
    fn decks_leagues_and_archetypes_count_listed_finished_matches() {
        let (_dir, conn, seeded) = seeded("console-views-counts");
        assert_eq!(
            text(
                &conn,
                &format!(
                    "SELECT platform || ',' || matches || ',' || wins || ',' || losses
                     || ',' || archived FROM v_decks WHERE deck_id = {}",
                    seeded.endstep_deck
                )
            )
            .as_deref(),
            Some("endstep,1,1,0,0")
        );
        assert_eq!(
            text(
                &conn,
                "SELECT matches_as_me || ',' || matches_against FROM v_archetypes
                 WHERE name = 'Rogue'"
            )
            .as_deref(),
            Some("0,1")
        );
        assert_eq!(
            text(
                &conn,
                "SELECT matches_as_me || ',' || matches_against FROM v_archetypes
                 WHERE name = 'Homebrew'"
            )
            .as_deref(),
            Some("1,0")
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(row, ';') FROM (
                     SELECT league_id || ',' || name || ',' || kind || ',' || state
                         || ',' || wins || ',' || losses || ',' || started_at
                         || ',' || started_ms || ',' || COALESCE(deck_name, '-') AS row
                     FROM v_leagues ORDER BY league_id)"
            )
            .as_deref(),
            Some(
                "1,Qleague,constructed,complete,1,0,2026-08-17T20:53:20Z,1787000000000,\
                 Qendstep deck;\
                 4,Qcourse,constructed,complete,3,2,2026-08-17T20:53:20Z,1787000000000,-"
            )
        );
        // A match in a detached league is in no league.
        assert_eq!(
            int(
                &conn,
                &format!(
                    "SELECT league_id FROM v_matches WHERE match_id = {}",
                    seeded.arena
                )
            ),
            None
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(name || ',' || platform, ';') FROM (
                     SELECT name, platform FROM v_decks
                     WHERE name IN ('Qpulled', 'Qfile') ORDER BY name)"
            )
            .as_deref(),
            Some("Qfile,mtgo;Qpulled,arena")
        );
        assert_eq!(
            int(
                &conn,
                &format!(
                    "SELECT league_id FROM v_matches WHERE match_id = {}",
                    seeded.endstep
                )
            ),
            Some(1)
        );
        // A match in a deleted league is in no league.
        assert_eq!(
            int(
                &conn,
                &format!(
                    "SELECT league_id FROM v_matches WHERE match_id = {}",
                    seeded.mtgo
                )
            ),
            None
        );
        assert_eq!(
            int(
                &conn,
                "SELECT COUNT(*) FROM v_cards WHERE catalog_id = 1001"
            ),
            Some(1)
        );
    }

    /// A `game_cards` row of `side` with `values` set (the rest default to
    /// 0). `kept` sets the opening and the starting hand.
    fn card_row(
        conn: &Connection,
        game_id: i64,
        side: &str,
        catalog_id: u32,
        values: &[(&str, i64)],
    ) {
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![game_id, side, catalog_id],
        )
        .expect("game card");
        for (column, value) in values {
            let set = if *column == "kept" {
                "opening_hand = ?1, starting_hand = ?1".to_string()
            } else {
                format!("{column} = ?1")
            };
            conn.execute(
                &format!(
                    "UPDATE game_cards SET {set}
                     WHERE game_id = ?2 AND side = ?3 AND catalog_id = ?4"
                ),
                rusqlite::params![value, game_id, side, catalog_id],
            )
            .expect(column);
        }
    }

    /// A match of `version_id`, in `state`, with one game per result.
    fn card_match(
        conn: &Connection,
        token: &str,
        version_id: i64,
        state: &str,
        results: &[&str],
    ) -> Vec<i64> {
        let match_id: i64 = conn
            .query_row(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version, deck_version_id, format)
                 VALUES (?1, (SELECT MIN(id) FROM accounts), 'log', ?2, 'win', 1000, 1000, 1, 1,
                     ?3, 'CMODERN')
                 RETURNING id",
                rusqlite::params![token, state, version_id],
                |row| row.get(0),
            )
            .expect("match");
        let mut games = Vec::new();
        for (index, result) in results.iter().enumerate() {
            let game_number = index as i64 + 1;
            let game_id: i64 = conn
                .query_row(
                    "INSERT INTO games (match_id, game_number, result, end_reason, on_play)
                     VALUES (?1, ?2, ?3, 'win', ?2 % 2) RETURNING id",
                    rusqlite::params![match_id, game_number, result],
                    |row| row.get(0),
                )
                .expect("game");
            games.push(game_id);
        }
        games
    }

    /// One `v_card_games` count for `key` on `side` in `deck_id`.
    fn card_count(conn: &Connection, deck_id: i64, side: &str, key: &str, expression: &str) -> u32 {
        conn.query_row(
            &format!(
                "SELECT COALESCE({expression}, 0) FROM v_card_games
                 WHERE side = ?1 AND deck_id = ?2 AND card_key = ?3"
            ),
            rusqlite::params![side, deck_id, key],
            |row| row.get(0),
        )
        .expect(expression)
    }

    /// The distinct values of `column` in `v_card_games` for `deck_id` on
    /// `side`, for `key` (or every key), sorted.
    fn card_values(
        conn: &Connection,
        deck_id: i64,
        side: &str,
        key: Option<&str>,
        column: &str,
    ) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT DISTINCT {column} FROM v_card_games
                 WHERE side = ?1 AND deck_id = ?2 AND (?3 IS NULL OR card_key = ?3)
                 ORDER BY 1"
            ))
            .expect(column);
        stmt.query_map(rusqlite::params![side, deck_id, key], |row| row.get(0))
            .expect(column)
            .map(|value| value.expect(column))
            .collect()
    }

    /// Every card key on `side` in `deck_id`.
    fn card_keys(conn: &Connection, deck_id: i64, side: &str) -> Vec<String> {
        card_values(conn, deck_id, side, None, "card_key")
    }

    /// Every name `key` has on `side` in `deck_id` (one, when the rows agree).
    fn card_names(conn: &Connection, deck_id: i64, side: &str, key: &str) -> Vec<String> {
        card_values(conn, deck_id, side, Some(key), "card_name")
    }

    #[test]
    fn card_games_agree_with_card_stats() {
        use crate::card_stats::{CardStatsFilter, card_stats};

        let (dir, conn) = db("console-views-card-games");
        crate::test_support::account(&conn, "Qlocal");
        let deck_id: i64 = conn
            .query_row(
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at,
                     source)
                 VALUES ('q-cards', 1, 'Qcards', 'CMODERN', 0, 0, 'file') RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("deck");
        let version_id: i64 = conn
            .query_row(
                "INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES (?1, 'SIG-Q', 0) RETURNING id",
                [deck_id],
                |row| row.get(0),
            )
            .expect("version");
        // Two printings of Qbolt; Qland and a variant printing of it (its
        // own catalog id, as MTGO gives some variants); a split card and a
        // stub for its face (no oracle id, named by the log); a sideboard
        // card; a stub that is no card's face (a token, say). Catalog id
        // 2999 has no card data at all.
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, type) VALUES
                 (2001, 'o-bolt', 'Qbolt', 'Instant'),
                 (2002, 'o-bolt', 'Qbolt', 'Instant'),
                 (2003, 'o-land', 'Qland', 'Land'),
                 (2004, 'o-land', 'Qland', 'Land'),
                 (2006, 'o-split', 'Qsplit // Qface', 'Sorcery'),
                 (2007, NULL, 'Qface', NULL),
                 (2008, 'o-side', 'Qsideboard', 'Creature'),
                 (2009, NULL, 'Qtoken', NULL);",
        )
        .expect("cards");

        let first = card_match(&conn, "q-first", version_id, "complete", &["win", "loss"]);
        let second = card_match(&conn, "q-second", version_id, "complete", &["draw"]);
        let live = card_match(&conn, "q-live", version_id, "in_progress", &["win"]);
        let hidden = card_match(&conn, "q-hidden", version_id, "complete", &["win"]);
        hide(&conn, "q-hidden");
        conn.execute_batch(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             SELECT id, 0, 1, 1.0, 1 FROM matches WHERE token = 'q-first'
             UNION ALL SELECT id, 1, 2, 1.0, 1 FROM matches WHERE token = 'q-first';",
        )
        .expect("archetypes");

        // Game 1 of the first match.
        let game = first[0];
        card_row(
            &conn,
            game,
            "local",
            2001,
            &[("main_qty", 2), ("kept", 1), ("seen", 1), ("cast", 1)],
        );
        card_row(&conn, game, "local", 2002, &[("main_qty", 2), ("seen", 1)]);
        card_row(
            &conn,
            game,
            "local",
            2003,
            &[("main_qty", 4), ("kept", 2), ("seen", 3), ("played", 2)],
        );
        card_row(&conn, game, "local", 2999, &[("main_qty", 1)]);
        card_row(&conn, game, "local", 2006, &[("main_qty", 1), ("seen", 1)]);
        // The face's cast folds into the split card; the token is dropped.
        card_row(&conn, game, "local", 2007, &[("seen", 1), ("cast", 1)]);
        card_row(&conn, game, "local", 2009, &[("seen", 1), ("cast", 1)]);
        // A game 1 sideboard card: not a deck row, counted by neither.
        card_row(&conn, game, "local", 2008, &[("side_qty", 1)]);
        // The opponent's face folds into the split card; their token stays.
        card_row(&conn, game, "opponent", 2003, &[("seen", 1)]);
        card_row(&conn, game, "opponent", 2007, &[("seen", 1)]);
        card_row(&conn, game, "opponent", 2009, &[("seen", 1)]);
        // Game 2: the sideboard card is a deck row postboard.
        let game = first[1];
        card_row(&conn, game, "local", 2001, &[("main_qty", 4)]);
        card_row(
            &conn,
            game,
            "local",
            2003,
            &[("main_qty", 4), ("kept", 1), ("seen", 2)],
        );
        card_row(&conn, game, "local", 2008, &[("side_qty", 1)]);
        card_row(
            &conn,
            game,
            "local",
            2999,
            &[("main_qty", 1), ("kept", 1), ("seen", 1), ("cast", 1)],
        );
        // A draw: the variant printing was seen, kept and cast, the deck's
        // printing was not.
        let game = second[0];
        card_row(
            &conn,
            game,
            "local",
            2002,
            &[("main_qty", 4), ("seen", 2), ("cast", 2)],
        );
        card_row(&conn, game, "local", 2003, &[("main_qty", 4)]);
        // The split card is not in the deck this game: its face is dropped.
        card_row(&conn, game, "local", 2007, &[("seen", 1), ("cast", 1)]);
        card_row(
            &conn,
            game,
            "local",
            2004,
            &[("kept", 1), ("seen", 1), ("cast", 1)],
        );
        // Neither an in-progress nor a hidden match is read.
        card_row(
            &conn,
            live[0],
            "local",
            2001,
            &[("main_qty", 4), ("seen", 1)],
        );
        card_row(
            &conn,
            hidden[0],
            "local",
            2001,
            &[("main_qty", 4), ("seen", 1)],
        );

        let stats = card_stats(&conn, deck_id, &CardStatsFilter::all()).expect("card stats");
        let opponent_filter = CardStatsFilter {
            side: game_detail::Side::Opponent,
            ..CardStatsFilter::all()
        };
        let opponent_stats = card_stats(&conn, deck_id, &opponent_filter).expect("opponent");
        drop(conn);
        let conn = open_console(&dir.join("mymtgo.sqlite")).expect("console");

        let mut tab_keys: Vec<String> = stats.iter().map(|stat| stat.key.clone()).collect();
        tab_keys.sort();
        assert_eq!(
            tab_keys,
            vec!["#2999", "o-bolt", "o-land", "o-side", "o-split"]
        );
        for stat in &stats {
            let key = stat.key.as_str();
            assert_eq!(
                card_count(&conn, deck_id, "me", key, "COUNT(*)"),
                stat.total_games,
                "{key} games"
            );
            assert_eq!(
                card_count(&conn, deck_id, "me", key, "SUM(in_starting_hand > 0)"),
                stat.kept_games,
                "{key} kept"
            );
            assert_eq!(
                card_count(&conn, deck_id, "me", key, "SUM(seen > 0)"),
                stat.seen_games,
                "{key} seen"
            );
            assert_eq!(
                card_count(&conn, deck_id, "me", key, "SUM(casts > 0)"),
                stat.cast_games,
                "{key} cast"
            );
            assert_eq!(
                card_names(&conn, deck_id, "me", key),
                vec![stat.name.clone()]
            );
        }
        assert_eq!(
            stats
                .iter()
                .find(|stat| stat.key == "#2999")
                .map(|stat| stat.name.as_str()),
            Some("Card 2999")
        );
        // The face folded and the token left out, as on the tab.
        assert_eq!(card_keys(&conn, deck_id, "me"), tab_keys);

        // The opponent's cards: the same keys (the face folded, the token
        // kept) and names. Their games are the games each card was shown,
        // where the tab counts every game, so only seen and cast compare.
        let mut opponent_keys: Vec<String> =
            opponent_stats.iter().map(|stat| stat.key.clone()).collect();
        opponent_keys.sort();
        assert_eq!(opponent_keys, vec!["#2009", "o-land", "o-split"]);
        assert_eq!(card_keys(&conn, deck_id, "opponent"), opponent_keys);
        for stat in &opponent_stats {
            let key = stat.key.as_str();
            assert_eq!(
                card_count(&conn, deck_id, "opponent", key, "SUM(seen > 0)"),
                stat.seen_games,
                "opponent {key} seen"
            );
            assert_eq!(
                card_count(&conn, deck_id, "opponent", key, "SUM(casts > 0)"),
                stat.cast_games,
                "opponent {key} cast"
            );
            assert_eq!(
                card_names(&conn, deck_id, "opponent", key),
                vec![stat.name.clone()]
            );
        }

        // Printings add up into one row per game; the seat columns.
        assert_eq!(
            text(
                &conn,
                "SELECT game_number || ',' || postboard || ',' || won || ',' || on_play || ','
                     || platform || ',' || format || ',' || deck_name || ',' || archetype || ','
                     || opponent_archetype || ',' || card_name || ',' || card_type || ','
                     || in_deck || ',' || in_starting_hand || ',' || seen || ',' || casts
                 FROM v_card_games WHERE side = 'me' AND card_key = 'o-bolt' AND game_number = 1
                     AND won = 1"
            )
            .as_deref(),
            Some("1,0,1,1,mtgo,CMODERN,Qcards,Homebrew,Rogue,Qbolt,Instant,4,1,2,1")
        );
        // A draw is not won.
        assert_eq!(
            int(
                &conn,
                "SELECT MAX(won) FROM v_card_games WHERE card_key = 'o-bolt' AND casts = 2"
            ),
            Some(0)
        );
    }

    #[test]
    fn card_games_read_the_card_stats_rows() {
        // The view inlines the tab's row rule; it must stay the same text.
        let squash = |sql: &str| sql.split_whitespace().collect::<Vec<_>>().join(" ");
        let view = ALL
            .iter()
            .find(|view| view.name == "v_card_games")
            .expect("v_card_games");
        for rule in [
            crate::card_stats::deck_row("gc"),
            crate::card_stats::other_row("gc"),
        ] {
            assert!(squash(view.sql).contains(&squash(&rule)), "{rule}");
        }
    }
}
