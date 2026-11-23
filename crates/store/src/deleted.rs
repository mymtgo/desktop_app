//! The Deleted lists of Settings, Archives (league self-management spec
//! 5.5): the leagues and matches the player deleted, newest deletion first,
//! every account's of one platform. Restore is `LeagueEdit::Restore` and
//! `MatchEdit::Restore`.

use rusqlite::Connection;

use crate::StoreError;
use crate::leagues::DELETED_WITH_LEAGUE;
use crate::platform::Platform;

/// A deleted league as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedLeague {
    pub run_key: String,
    pub name: String,
    pub started_at: i64,
    pub removed_at: i64,
    pub wins: u32,
    pub losses: u32,
    /// The league's deck: its `deck_id`, else the deck of its deck version.
    pub deck_name: Option<String>,
    /// The league's format, for a league with no deck.
    pub format: Option<String>,
}

/// A deleted match as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedMatch {
    pub token: String,
    pub started_at: i64,
    pub hidden_at: i64,
    pub opponent: Option<String>,
    /// The match outcome: `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    /// The deck the match was played with (via its deck version).
    pub deck_name: Option<String>,
    /// The league the match was in (see `deleted_matches_sql`).
    pub league_name: Option<String>,
    /// Whether that league is itself deleted.
    pub league_removed: bool,
    /// The deleted league this match was deleted together with
    /// (`leagues::DELETED_WITH_LEAGUE`): the match cannot be restored on its
    /// own, only with the league (issue #418).
    pub deleted_with_league: Option<DeletedWithLeague>,
}

/// The deleted league a deleted match was deleted together with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedWithLeague {
    pub run_key: String,
    pub name: String,
}

/// Both Deleted lists.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletedItems {
    pub leagues: Vec<DeletedLeague>,
    pub matches: Vec<DeletedMatch>,
}

/// Removed leagues, detached ones too (a fully deleted run's row stays
/// detached), with the record of their matches: those still pointing at the
/// league, and those its "Delete them too" hid (`DELETED_WITH_LEAGUE`; the
/// league sync has cleared their league). Leagues are MTGO's: one without an
/// account counts as MTGO. The platform is `?1`.
fn deleted_leagues_sql() -> String {
    format!(
        "SELECT l.run_key, l.name, l.started_at, l.removed_at,
            (SELECT COUNT(*) FROM matches m
             WHERE m.detached_at IS NULL AND m.outcome = 'win'
               AND (m.league_id = l.id OR ({DELETED_WITH_LEAGUE}))),
            (SELECT COUNT(*) FROM matches m
             WHERE m.detached_at IS NULL AND m.outcome = 'loss'
               AND (m.league_id = l.id OR ({DELETED_WITH_LEAGUE}))),
            d.name, l.format
        FROM leagues l
        LEFT JOIN deck_versions dv ON dv.id = l.deck_version_id
        LEFT JOIN decks d ON d.id = COALESCE(l.deck_id, dv.deck_id)
        WHERE l.removed_at IS NOT NULL
          AND COALESCE((SELECT a.platform FROM accounts a WHERE a.id = l.account_id), 'mtgo') = ?1
        ORDER BY l.removed_at DESC, l.id DESC"
    )
}

/// Hidden matches of the platform's (`?1`) accounts the projection still
/// derives (a detached one has nothing to restore). Each names the league it
/// was in when it was deleted
/// (`ml`, `deleted_from_league_id`; spec section 13), and that league again
/// as `l` when the match was deleted together with it
/// (`DELETED_WITH_LEAGUE`).
fn deleted_matches_sql() -> String {
    format!(
        "SELECT m.token, m.started_at, m.hidden_at, COALESCE(o.display_name, o.username),
            m.outcome, d.name,
            ml.name, ml.removed_at IS NOT NULL, l.run_key, l.name
        FROM matches m LEFT JOIN opponents o ON o.id = m.opponent_id
        LEFT JOIN deck_versions dv ON dv.id = m.deck_version_id
        LEFT JOIN decks d ON d.id = dv.deck_id
        LEFT JOIN leagues ml ON ml.id = m.deleted_from_league_id
        LEFT JOIN leagues l ON {DELETED_WITH_LEAGUE}
        WHERE m.hidden_at IS NOT NULL AND m.detached_at IS NULL
          AND m.account_id IN (SELECT id FROM accounts WHERE platform = ?1)
        ORDER BY m.hidden_at DESC, m.id DESC"
    )
}

/// The platform's Deleted lists: two queries.
pub fn deleted_items(conn: &Connection, platform: Platform) -> Result<DeletedItems, StoreError> {
    let mut items = DeletedItems::default();
    {
        let mut stmt = conn.prepare_cached(&deleted_leagues_sql())?;
        let rows = stmt.query_map([platform], |row| {
            Ok(DeletedLeague {
                run_key: row.get(0)?,
                name: row.get(1)?,
                started_at: row.get(2)?,
                removed_at: row.get(3)?,
                wins: row.get(4)?,
                losses: row.get(5)?,
                deck_name: row.get(6)?,
                format: row.get(7)?,
            })
        })?;
        for row in rows {
            items.leagues.push(row?);
        }
    }
    let mut stmt = conn.prepare_cached(&deleted_matches_sql())?;
    let rows = stmt.query_map([platform], |row| {
        let run_key: Option<String> = row.get(8)?;
        let name: Option<String> = row.get(9)?;
        let deleted_with_league = match (run_key, name) {
            (Some(run_key), Some(name)) => Some(DeletedWithLeague { run_key, name }),
            _ => None,
        };
        Ok(DeletedMatch {
            token: row.get(0)?,
            started_at: row.get(1)?,
            hidden_at: row.get(2)?,
            opponent: row.get(3)?,
            result: row.get(4)?,
            deck_name: row.get(5)?,
            league_name: row.get(6)?,
            league_removed: row.get(7)?,
            deleted_with_league,
        })
    })?;
    for row in rows {
        items.matches.push(row?);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::{insert_manual_league, link_league_match};
    use fixture_tools::TestDir;
    use rusqlite::params;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qopp', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES (1, 'q-deck', 1, 'Qburn', 'Modern', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (1, 1, 'q-sig', 0);",
        )
        .expect("registries");
        (dir, conn)
    }

    /// A complete log match of Qlocal against Qopp.
    fn played(conn: &Connection, token: &str, started_at: i64, outcome: &str) {
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES (?1, 1, (SELECT id FROM opponents WHERE username = 'Qopp'), 'log',
                 'complete', ?3, ?2, ?2, 1, 1)",
            params![token, started_at, outcome],
        )
        .expect("match");
    }

    #[test]
    fn the_deleted_lists_hold_removed_leagues_and_hidden_matches_newest_first() {
        let (_dir, conn) = db("deleted-lists");
        for (token, started_at, outcome) in [
            ("q-a", 100, "win"),
            ("q-b", 200, "loss"),
            ("q-c", 300, "win"),
            ("q-d", 400, "loss"),
            ("q-e", 500, "win"),
            ("q-f", 450, "win"),
        ] {
            played(&conn, token, started_at, outcome);
        }
        insert_manual_league(&conn, 1, Some(1), None, "Q kept", 10, 10);
        insert_manual_league(&conn, 2, Some(1), Some(1), "Q too", 20, 20);
        insert_manual_league(&conn, 3, Some(1), None, "Q listed", 30, 30);
        conn.execute_batch(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (9, 'log:q-token', 1, 'log', 'constructed', 'active', 'q-token', 'Q log',
                 50, 50);
             UPDATE matches SET league_token = 'q-token', league_id = 9 WHERE token = 'q-d';",
        )
        .expect("a log league");
        link_league_match(&conn, 1, "q-a");
        link_league_match(&conn, 1, "q-b");
        conn.execute("UPDATE matches SET league_id = 2 WHERE token = 'q-c'", [])
            .expect("in league 2");
        // League 1 deleted keeping its matches; league 2 with "Delete them
        // too" (its match hidden with its time); q-d deleted on its own from
        // the log league; q-f deleted on its own, in no league, at league
        // 1's time; q-e hidden but detached. The league sync then clears the
        // league of the hidden log matches.
        conn.execute_batch(
            "UPDATE leagues SET removed_at = 700 WHERE id = 1;
             UPDATE leagues SET removed_at = 800 WHERE id = 2;
             UPDATE leagues SET format = 'Pauper' WHERE id = 1;
             UPDATE matches SET deck_version_id = 1 WHERE token = 'q-c';
             UPDATE matches SET hidden_at = 800 WHERE token = 'q-c';
             UPDATE matches SET hidden_at = 600 WHERE token = 'q-d';
             UPDATE matches SET hidden_at = 700 WHERE token = 'q-f';
             UPDATE matches SET hidden_at = 650, detached_at = 650 WHERE token = 'q-e';
             UPDATE matches SET league_id = NULL WHERE token IN ('q-c', 'q-d');",
        )
        .expect("deletes");

        let found = deleted_items(&conn, Platform::Mtgo).expect("deleted");
        assert_eq!(
            found.leagues,
            vec![
                DeletedLeague {
                    run_key: "manual:2".to_string(),
                    name: "Q too".to_string(),
                    started_at: 20,
                    removed_at: 800,
                    wins: 1,
                    losses: 0,
                    deck_name: Some("Qburn".to_string()),
                    format: Some("Modern".to_string()),
                },
                DeletedLeague {
                    run_key: "manual:1".to_string(),
                    name: "Q kept".to_string(),
                    started_at: 10,
                    removed_at: 700,
                    wins: 1,
                    losses: 1,
                    deck_name: None,
                    format: Some("Pauper".to_string()),
                },
            ]
        );
        let hidden: Vec<(&str, i64)> = found
            .matches
            .iter()
            .map(|found| (found.token.as_str(), found.hidden_at))
            .collect();
        assert_eq!(hidden, vec![("q-c", 800), ("q-f", 700), ("q-d", 600)]);
        assert_eq!(found.matches[0].opponent.as_deref(), Some("Qopp"));
        assert_eq!(found.matches[0].deck_name.as_deref(), Some("Qburn"));
        assert_eq!(found.matches[2].deck_name, None);
        // Each names the league it was in when it was deleted: q-c league 2
        // (removed), q-d the log league (listed), q-f none.
        assert_eq!(found.matches[0].league_name.as_deref(), Some("Q too"));
        assert!(found.matches[0].league_removed);
        assert_eq!(found.matches[2].league_name.as_deref(), Some("Q log"));
        assert!(!found.matches[2].league_removed);
        assert_eq!(found.matches[1].league_name, None);
        assert!(!found.matches[1].league_removed);
        // Issue #418: q-c was deleted together with league 2, so it names
        // that league for "Restore league". q-f was deleted at league 1's
        // time but was in no league: deleted alone, as q-d.
        assert_eq!(
            found.matches[0].deleted_with_league,
            Some(DeletedWithLeague {
                run_key: "manual:2".to_string(),
                name: "Q too".to_string(),
            })
        );
        assert_eq!(found.matches[1].deleted_with_league, None);
        assert_eq!(found.matches[2].deleted_with_league, None);
        assert_eq!(
            (
                found.matches[2].started_at,
                found.matches[2].result.as_str()
            ),
            (400, "loss")
        );
    }

    #[test]
    fn the_deleted_list_reads_one_platform() {
        let (_dir, conn) = db("deleted-arena");
        played(&conn, "q-mtgo", 100, "win");
        played(&conn, "q-arena", 200, "loss");
        crate::test_support::move_match_to_arena(&conn, "q-arena");
        crate::test_support::hide(&conn, "q-mtgo");
        crate::test_support::hide(&conn, "q-arena");
        insert_manual_league(&conn, 1, Some(1), None, "Q mtgo league", 10, 10);
        conn.execute("UPDATE leagues SET removed_at = 700 WHERE id = 1", [])
            .expect("delete league");

        let items = deleted_items(&conn, Platform::Mtgo).expect("deleted");
        let tokens: Vec<&str> = items.matches.iter().map(|m| m.token.as_str()).collect();
        assert_eq!(tokens, ["q-mtgo"]);
        assert_eq!(items.leagues.len(), 1);
        let items = deleted_items(&conn, Platform::Arena).expect("deleted");
        let tokens: Vec<&str> = items.matches.iter().map(|m| m.token.as_str()).collect();
        assert_eq!(tokens, ["q-arena"]);
        assert!(items.leagues.is_empty(), "leagues are MTGO's");
    }

    #[test]
    fn nothing_deleted_reads_empty() {
        let (_dir, conn) = db("deleted-none");
        played(&conn, "q-a", 100, "win");
        assert_eq!(
            deleted_items(&conn, Platform::Mtgo).expect("deleted"),
            DeletedItems::default()
        );
    }
}
