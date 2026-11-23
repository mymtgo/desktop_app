//! The stored rows behind the match detail page (0.x `BuildMatchShowProps`):
//! the match with its account, opponent, deck version and note, and its
//! games. The per-game detail itself (hands, reveals, sideboard) is in each
//! game's card rows (`game_sync::match_game_cards`); the opponent's colours
//! on the match lists come from the same rows (`opponent_color_identities`).

use rusqlite::{Connection, OptionalExtension};

use crate::StoreError;
use crate::leagues::SHOWN_IN_A_LEAGUE;

/// The deck version a match is linked to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchDeck {
    pub deck_id: i64,
    pub name: String,
    /// Comma separated colours (`U,R`).
    pub color_identity: Option<String>,
    pub version_id: i64,
    /// The version's card list (`decks::decode_signature`).
    pub signature: String,
}

/// One listed match (a hidden one is never read).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchHeader {
    pub id: i64,
    pub token: String,
    /// `log` or `imported`.
    pub source: String,
    pub account_id: i64,
    /// The local player.
    pub account: String,
    /// The platform of the match's account.
    pub platform: crate::Platform,
    /// The opponent's display name: an Arena opponent's screen name, else
    /// the username.
    pub opponent: Option<String>,
    pub format: Option<String>,
    pub match_type: Option<String>,
    pub state: String,
    pub outcome: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub league_token: Option<String>,
    /// In a league run (`league_id`) or with a league token (`match_list`'s
    /// "league" type).
    pub in_league: bool,
    /// The name of the constructed league run holding the match (a derived,
    /// manual or imported run; the Leagues page's runs).
    pub league_name: Option<String>,
    pub notes: Option<String>,
    pub deck: Option<MatchDeck>,
    /// The player fixed the match ("Edit match": `corrected_at` is set), so
    /// the page shows "Set by you".
    pub corrected: bool,
    /// Complete, and archetype detection has not decided it yet
    /// (`archetypes_detected_at` unset): it may still name the archetypes.
    pub detection_pending: bool,
}

/// One stored game of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRow {
    /// The `games.id` its card rows hang off.
    pub id: i64,
    pub game_number: u32,
    pub mtgo_game_id: Option<u64>,
    /// `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    pub on_play: Option<bool>,
    pub local_mulligans: Option<u32>,
    pub opp_mulligans: Option<u32>,
    pub turn_count: Option<u32>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

/// The header read; a deleted league is no league (league self-management
/// spec 3.3).
fn header_sql() -> String {
    format!(
        "
SELECT m.id, m.token, m.source, m.account_id, a.username, COALESCE(o.display_name, o.username),
       m.format, m.match_type,
       m.state, m.outcome, m.started_at, m.ended_at, m.league_token,
       {SHOWN_IN_A_LEAGUE},
       m.notes,
       d.id, d.name, d.color_identity, v.id, v.signature, l.name,
       m.corrected_at IS NOT NULL,
       m.state = 'complete' AND m.archetypes_detected_at IS NULL,
       a.platform
FROM listed_matches m
JOIN accounts a ON a.id = m.account_id
LEFT JOIN listed_leagues l ON l.id = m.league_id AND l.detached_at IS NULL
    AND l.kind = 'constructed'
LEFT JOIN opponents o ON o.id = m.opponent_id
LEFT JOIN deck_versions v ON v.id = m.deck_version_id
LEFT JOIN decks d ON d.id = v.deck_id
WHERE m.token = ?1"
    )
}

/// The match `token`; `None` when there is none or it is hidden.
pub fn match_header(conn: &Connection, token: &str) -> Result<Option<MatchHeader>, StoreError> {
    let mut stmt = conn.prepare_cached(&header_sql())?;
    let header = stmt
        .query_row([token], |row| {
            let deck_id: Option<i64> = row.get(15)?;
            let deck = match deck_id {
                Some(deck_id) => Some(MatchDeck {
                    deck_id,
                    name: row.get(16)?,
                    color_identity: row.get(17)?,
                    version_id: row.get(18)?,
                    signature: row.get(19)?,
                }),
                None => None,
            };
            Ok(MatchHeader {
                id: row.get(0)?,
                token: row.get(1)?,
                source: row.get(2)?,
                account_id: row.get(3)?,
                account: row.get(4)?,
                opponent: row.get(5)?,
                format: row.get(6)?,
                match_type: row.get(7)?,
                state: row.get(8)?,
                outcome: row.get(9)?,
                started_at_ms: row.get(10)?,
                ended_at_ms: row.get(11)?,
                league_token: row.get(12)?,
                in_league: row.get(13)?,
                league_name: row.get(20)?,
                notes: row.get(14)?,
                deck,
                corrected: row.get(21)?,
                detection_pending: row.get(22)?,
                platform: row.get(23)?,
            })
        })
        .optional()?;
    Ok(header)
}

/// The games of match row `match_id`, by game number.
pub fn match_games(conn: &Connection, match_id: i64) -> Result<Vec<GameRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT game_number, mtgo_game_id, result, on_play, local_mulligans, opp_mulligans,
                turn_count, started_at, ended_at, id
         FROM games WHERE match_id = ?1 ORDER BY game_number, id",
    )?;
    let rows = stmt.query_map([match_id], |row| {
        let game_id: Option<i64> = row.get(1)?;
        Ok(GameRow {
            id: row.get(9)?,
            game_number: row.get(0)?,
            mtgo_game_id: game_id.map(|id| id as u64),
            result: row.get(2)?,
            on_play: row.get(3)?,
            local_mulligans: row.get(4)?,
            opp_mulligans: row.get(5)?,
            turn_count: row.get(6)?,
            started_at_ms: row.get(7)?,
            ended_at_ms: row.get(8)?,
        })
    })?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row?);
    }
    Ok(games)
}

/// The colour identities (`cards.color_identity`, `B,G`) of every card the
/// opponent showed in any game of the listed match `token`, one per card.
/// Empty for an unknown or hidden match, or one whose cards have no data.
pub fn opponent_color_identities(
    conn: &Connection,
    token: &str,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT c.catalog_id, c.color_identity
         FROM listed_matches m
         JOIN games g ON g.match_id = m.id
         JOIN game_cards gc ON gc.game_id = g.id AND gc.side = 'opponent' AND gc.seen > 0
         JOIN cards c ON c.catalog_id = gc.catalog_id
         WHERE m.token = ?1 AND c.color_identity IS NOT NULL
         ORDER BY c.catalog_id",
    )?;
    let rows = stmt.query_map([token], |row| row.get::<_, String>(1))?;
    let mut identities = Vec::new();
    for row in rows {
        identities.push(row?);
    }
    Ok(identities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::{OTHER, TOKEN, hide, insert_manual_league, link_league_match, view};
    use crate::write::{MatchWrite, WriteBatch, commit};
    use fixture_tools::TestDir;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let mut league = view(OTHER, "Qother", 2_000);
        league.league_token = Some("q-league".to_string());
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(league),
                        revision: 1,
                    },
                ],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        (dir, conn)
    }

    #[test]
    fn the_header_has_the_players_deck_note_and_league_mark() {
        let (_dir, conn) = db("match-detail-header");
        conn.execute_batch(&format!(
            "INSERT INTO decks (id, mtgo_id, name, format, color_identity, created_at, updated_at, source)
                 VALUES (5, 'q-deck', 'Qburn', 'CMODERN', 'R', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (10, 5, 'SIG', 0);
             UPDATE matches SET deck_version_id = 10, notes = 'Q note' WHERE token = '{TOKEN}';"
        ))
        .expect("seed");
        let header = match_header(&conn, TOKEN).expect("read").expect("found");
        assert_eq!(header.account, "Qlocal");
        assert_eq!(header.opponent.as_deref(), Some("Qopponent"));
        assert_eq!(
            (header.source.as_str(), header.state.as_str()),
            ("log", "complete")
        );
        assert_eq!(header.notes.as_deref(), Some("Q note"));
        assert!(!header.in_league);
        assert_eq!(
            header.deck,
            Some(MatchDeck {
                deck_id: 5,
                name: "Qburn".to_string(),
                color_identity: Some("R".to_string()),
                version_id: 10,
                signature: "SIG".to_string(),
            })
        );
        let league = match_header(&conn, OTHER).expect("read").expect("found");
        assert!(league.in_league);
        assert_eq!(league.deck, None);
        assert_eq!(match_header(&conn, "q-none").expect("read"), None);
    }

    // #503: a complete match the detection has not decided yet is pending,
    // so the page does not call it "Unknown archetype".
    #[test]
    fn a_complete_match_not_yet_detected_is_pending() {
        let (_dir, conn) = db("match-detail-detection-pending");
        let header = match_header(&conn, TOKEN).expect("read").expect("found");
        assert!(header.detection_pending);
        conn.execute(
            "UPDATE matches SET archetypes_detected_at = 5 WHERE token = ?1",
            [TOKEN],
        )
        .expect("detected");
        let header = match_header(&conn, TOKEN).expect("read").expect("found");
        assert!(!header.detection_pending);
    }

    #[test]
    fn match_detail_names_the_arena_opponent() {
        let (_dir, mut conn) = db("match-detail-arena-opponent");
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view("q-arena", "arena:u1", 3_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        crate::test_support::move_match_to_arena(&conn, "q-arena");
        crate::test_support::name_arena_opponent(&conn, "arena:u1", "Opponent");
        let header = match_header(&conn, "q-arena")
            .expect("read")
            .expect("found");
        assert_eq!(header.opponent.as_deref(), Some("Opponent"));
        let mtgo = match_header(&conn, TOKEN).expect("read").expect("found");
        assert_eq!(mtgo.opponent.as_deref(), Some("Qopponent"));
    }

    #[test]
    fn a_hidden_match_has_no_header() {
        let (_dir, conn) = db("match-detail-hidden");
        hide(&conn, TOKEN);
        assert_eq!(match_header(&conn, TOKEN).expect("read"), None);
    }

    #[test]
    fn games_come_by_number() {
        let (_dir, conn) = db("match-detail-games");
        let header = match_header(&conn, TOKEN).expect("read").expect("found");
        let games = match_games(&conn, header.id).expect("games");
        let numbers: Vec<(u32, Option<u64>, &str)> = games
            .iter()
            .map(|game| (game.game_number, game.mtgo_game_id, game.result.as_str()))
            .collect();
        assert_eq!(
            numbers,
            vec![
                (1, Some(7), "win"),
                (2, Some(8), "loss"),
                (3, Some(9), "win")
            ]
        );
        assert_eq!(games[0].on_play, Some(true));
        assert_eq!(games[0].turn_count, Some(9));
        assert_eq!(games[0].opp_mulligans, Some(1));
    }

    #[test]
    fn a_match_of_a_removed_league_reads_as_a_non_league_match() {
        let (_dir, conn) = db("match-detail-removed-league");
        insert_manual_league(&conn, 4, None, None, "Q manual", 0, 0);
        link_league_match(&conn, 4, TOKEN);
        let header = match_header(&conn, TOKEN).expect("read").expect("found");
        assert!(header.in_league);
        assert_eq!(header.league_name.as_deref(), Some("Q manual"));

        conn.execute("UPDATE leagues SET removed_at = 5 WHERE id = 4", [])
            .expect("removed");
        let header = match_header(&conn, TOKEN).expect("read").expect("found");
        assert!(!header.in_league);
        assert_eq!(header.league_name, None);

        conn.execute("UPDATE matches SET no_league = 1 WHERE token = ?1", [OTHER])
            .expect("taken out of every league");
        assert!(
            !match_header(&conn, OTHER)
                .expect("read")
                .expect("found")
                .in_league
        );
    }
}
