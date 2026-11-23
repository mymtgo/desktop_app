//! Arena course snapshots and match event names (Arena limited runs spec
//! section 2): the limited courses each `EventGetCoursesV2` response listed,
//! under the account logged in when it was read, and the event each Arena
//! match was played in. Both are facts, never edited or deleted.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::StoreError;

/// One course of one snapshot. Card ids are catalog ids (grpId plus
/// `ARENA_ID_OFFSET`). A snapshot with no course is one row with an empty
/// `course_id`, empty strings, zeros and no cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseObservation {
    pub file_id: i64,
    pub byte_offset: i64,
    pub observed_at_ms: i64,
    pub account_username: String,
    pub course_id: String,
    pub event_name: String,
    pub module: String,
    pub wins: u32,
    pub losses: u32,
    pub deck_id: String,
    pub deck_name: String,
    /// `(card id, copies)`.
    pub main: Vec<(u32, u32)>,
    pub side: Vec<(u32, u32)>,
    /// One card id per copy.
    pub pool: Vec<u32>,
}

/// The `cards` column: `{"main":[[id,n],..],"side":[[id,n],..],"pool":[id,..]}`.
#[derive(Serialize, Deserialize)]
struct Cards {
    main: Vec<(u32, u32)>,
    side: Vec<(u32, u32)>,
    pool: Vec<u32>,
}

/// Every catalog id a snapshot names: its deck, sideboard and pool.
fn observation_card_ids(observation: &CourseObservation) -> Vec<u32> {
    let mut ids: Vec<u32> = observation
        .main
        .iter()
        .chain(&observation.side)
        .map(|(catalog_id, _)| *catalog_id)
        .chain(observation.pool.iter().copied())
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The catalog ids in a stored snapshot's `cards` JSON; none when it does
/// not read.
pub(crate) fn stored_card_ids(json: &str) -> Vec<u32> {
    let Ok(cards) = serde_json::from_str::<Cards>(json) else {
        return Vec::new();
    };
    cards
        .main
        .iter()
        .chain(&cards.side)
        .map(|(catalog_id, _)| *catalog_id)
        .chain(cards.pool)
        .collect()
}

/// Stored against the Arena account named `account_username` (an untagged
/// name is the account holding its user id); a replayed
/// row (same file, offset and course) is skipped. With no such Arena account
/// the row is left out with a warning.
pub(crate) fn insert_observation(
    tx: &Transaction,
    observation: &CourseObservation,
) -> Result<(), StoreError> {
    let cards = serde_json::to_string(&Cards {
        main: observation.main.clone(),
        side: observation.side.clone(),
        pool: observation.pool.clone(),
    })?;
    let username = crate::arena::platform_account_username(tx, &observation.account_username)?;
    let inserted = tx
        .prepare_cached(
            "INSERT OR IGNORE INTO arena_course_observations (arena_log_file_id, byte_offset,
                 observed_at, account_id, course_id, event_name, module, wins, losses, deck_id,
                 deck_name, cards)
             SELECT ?1, ?2, ?3, id, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
             FROM accounts WHERE username = ?4 AND platform = 'arena'",
        )?
        .execute(params![
            observation.file_id,
            observation.byte_offset,
            observation.observed_at_ms,
            username,
            observation.course_id,
            observation.event_name,
            observation.module,
            observation.wins,
            observation.losses,
            observation.deck_id,
            observation.deck_name,
            cards,
        ])?;
    if inserted > 0 {
        // The pool's cards are stored nowhere else until a match is played
        // (a draft not yet played has only this): a stub makes the card
        // data pass ask about them.
        crate::cards::ensure_id_stubs(tx, &observation_card_ids(observation))?;
    }
    if inserted == 0 && !arena_account_exists(tx, &username)? {
        tracing::warn!(
            file_id = observation.file_id,
            byte_offset = observation.byte_offset,
            "course observation skipped: its Arena account is not stored"
        );
    }
    Ok(())
}

/// The event a match was played in; the first one stored for a match stays.
pub(crate) fn insert_match_event(
    tx: &Transaction,
    match_token: &str,
    event_name: &str,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT OR IGNORE INTO arena_match_events (match_token, event_name) VALUES (?1, ?2)",
    )?
    .execute(params![match_token, event_name])?;
    Ok(())
}

fn arena_account_exists(tx: &Transaction, username: &str) -> Result<bool, StoreError> {
    Ok(tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM accounts WHERE username = ?1 AND platform = 'arena')",
        [username],
        |row| row.get(0),
    )?)
}

const OBSERVATION_COLUMNS: &str = "o.arena_log_file_id, o.byte_offset, o.observed_at, a.username,
     o.course_id, o.event_name, o.module, o.wins, o.losses, o.deck_id, o.deck_name, o.cards";

/// The account's course observations in log order (a snapshot's courses by
/// course id).
pub fn observations(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<CourseObservation>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {OBSERVATION_COLUMNS}
         FROM arena_course_observations o JOIN accounts a ON a.id = o.account_id
         WHERE o.account_id = ?1
         ORDER BY o.arena_log_file_id, o.byte_offset, o.course_id"
    ))?;
    read_observations(&mut stmt, params![account_id])
}

/// The account's newest stored snapshot (its marker row when it was empty),
/// or nothing when none is stored.
pub fn latest_snapshot(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<CourseObservation>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {OBSERVATION_COLUMNS}
         FROM arena_course_observations o JOIN accounts a ON a.id = o.account_id
         WHERE (o.arena_log_file_id, o.byte_offset) = (
             SELECT arena_log_file_id, byte_offset FROM arena_course_observations
             WHERE account_id = ?1
             ORDER BY arena_log_file_id DESC, byte_offset DESC LIMIT 1)
           AND o.account_id = ?1
         ORDER BY o.course_id"
    ))?;
    read_observations(&mut stmt, params![account_id])
}

/// The pool of the course's newest observation.
pub fn latest_pool(conn: &Connection, course_id: &str) -> Result<Option<Vec<u32>>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT cards FROM arena_course_observations WHERE course_id = ?1
         ORDER BY arena_log_file_id DESC, byte_offset DESC LIMIT 1",
    )?;
    let mut rows = stmt.query([course_id])?;
    match rows.next()? {
        Some(row) => {
            let cards: Cards = serde_json::from_str(&row.get::<_, String>(0)?)?;
            Ok(Some(cards.pool))
        }
        None => Ok(None),
    }
}

/// The event name of each of the account's matches that has one, by token.
pub fn match_event_names(
    conn: &Connection,
    account_id: i64,
) -> Result<HashMap<String, String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT e.match_token, e.event_name
         FROM arena_match_events e JOIN matches m ON m.token = e.match_token
         WHERE m.account_id = ?1",
    )?;
    let rows = stmt.query_map([account_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every match token with a stored event name.
pub fn match_event_tokens(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT match_token FROM arena_match_events")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn read_observations(
    stmt: &mut rusqlite::CachedStatement,
    params: impl rusqlite::Params,
) -> Result<Vec<CourseObservation>, StoreError> {
    let mut rows = stmt.query(params)?;
    let mut observations = Vec::new();
    while let Some(row) = rows.next()? {
        let cards: Cards = serde_json::from_str(&row.get::<_, String>(11)?)?;
        observations.push(CourseObservation {
            file_id: row.get(0)?,
            byte_offset: row.get(1)?,
            observed_at_ms: row.get(2)?,
            account_username: row.get(3)?,
            course_id: row.get(4)?,
            event_name: row.get(5)?,
            module: row.get(6)?,
            wins: row.get(7)?,
            losses: row.get(8)?,
            deck_id: row.get(9)?,
            deck_name: row.get(10)?,
            main: cards.main,
            side: cards.side,
            pool: cards.pool,
        });
    }
    Ok(observations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Platform;
    use crate::arena::{PlatformAccount, insert_arena_log_file};
    use crate::test_support::{db, new_arena_log_file};
    use crate::write::{WriteBatch, commit};

    const PLAYER: &str = "Qplayer#12345";

    fn setup(label: &str) -> (fixture_tools::TestDir, Connection, i64) {
        let (dir, mut conn) = db(label);
        for path in ["/arena/Player-prev.log", "/arena/Player.log"] {
            insert_arena_log_file(&conn, &new_arena_log_file(path)).expect("file");
        }
        commit(
            &mut conn,
            &WriteBatch {
                platform_accounts: vec![PlatformAccount {
                    platform: Platform::Arena,
                    username: PLAYER.to_string(),
                    user_id: Some("QUSERID1".to_string()),
                    display_name: None,
                    seen_at_ms: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("account");
        let account_id = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = ?1",
                [PLAYER],
                |row| row.get(0),
            )
            .expect("account id");
        (dir, conn, account_id)
    }

    fn course(file_id: i64, byte_offset: i64, course_id: &str, wins: u32) -> CourseObservation {
        CourseObservation {
            file_id,
            byte_offset,
            observed_at_ms: 1_000 + byte_offset,
            account_username: PLAYER.to_string(),
            course_id: course_id.to_string(),
            event_name: "PremierDraft_FRA_20260929".to_string(),
            module: "CreateMatch".to_string(),
            wins,
            losses: 1,
            deck_id: "q-deck".to_string(),
            deck_name: "Draft Deck".to_string(),
            main: vec![(2_000_001, 2), (2_000_002, 1)],
            side: vec![(2_000_003, 1)],
            pool: vec![2_000_001, 2_000_001, 2_000_002, 2_000_003, wins],
        }
    }

    fn marker(file_id: i64, byte_offset: i64) -> CourseObservation {
        CourseObservation {
            file_id,
            byte_offset,
            observed_at_ms: 9_000,
            account_username: PLAYER.to_string(),
            course_id: String::new(),
            event_name: String::new(),
            module: String::new(),
            wins: 0,
            losses: 0,
            deck_id: String::new(),
            deck_name: String::new(),
            main: Vec::new(),
            side: Vec::new(),
            pool: Vec::new(),
        }
    }

    #[test]
    fn observations_round_trip_in_log_order() {
        let (_dir, mut conn, account_id) = setup("arena-courses-round-trip");
        let batch = WriteBatch {
            arena_course_observations: vec![
                course(2, 50, "q-course-b", 3),
                course(2, 50, "q-course-a", 0),
                marker(2, 10),
                course(1, 900, "q-course-a", 1),
                CourseObservation {
                    account_username: "Qnobody#00000".to_string(),
                    ..course(2, 70, "q-course-c", 0)
                },
            ],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("commit");
        commit(&mut conn, &batch).expect("a replay is skipped");

        assert_eq!(
            observations(&conn, account_id).expect("read"),
            vec![
                course(1, 900, "q-course-a", 1),
                marker(2, 10),
                course(2, 50, "q-course-a", 0),
                course(2, 50, "q-course-b", 3),
            ],
            "log order, a snapshot by course id, the unknown account left out"
        );
        assert_eq!(
            latest_snapshot(&conn, account_id).expect("latest"),
            vec![
                course(2, 50, "q-course-a", 0),
                course(2, 50, "q-course-b", 3)
            ]
        );
        assert_eq!(
            latest_pool(&conn, "q-course-a").expect("pool"),
            Some(vec![2_000_001, 2_000_001, 2_000_002, 2_000_003, 0])
        );
        assert_eq!(latest_pool(&conn, "q-missing").expect("pool"), None);
        let cards: String = conn
            .query_row(
                "SELECT cards FROM arena_course_observations WHERE course_id = ''",
                [],
                |row| row.get(0),
            )
            .expect("marker cards");
        assert_eq!(cards, r#"{"main":[],"side":[],"pool":[]}"#);
    }

    /// A bare fast login, its courses and a login line in one poll: the
    /// login renames the account before the snapshot, queued under the
    /// untagged name, is stored.
    #[test]
    fn a_snapshot_under_an_untagged_name_joins_the_renamed_account() {
        let (_dir, mut conn, _) = setup("arena-courses-renamed");
        let untagged = "arena:QUSERID2";
        commit(
            &mut conn,
            &WriteBatch {
                platform_accounts: vec![
                    PlatformAccount {
                        platform: Platform::Arena,
                        username: untagged.to_string(),
                        user_id: Some("QUSERID2".to_string()),
                        display_name: Some("Qother".to_string()),
                        seen_at_ms: 1,
                    },
                    PlatformAccount {
                        platform: Platform::Arena,
                        username: "Qother#00002".to_string(),
                        user_id: Some("QUSERID2".to_string()),
                        display_name: None,
                        seen_at_ms: 1,
                    },
                ],
                arena_course_observations: vec![CourseObservation {
                    account_username: untagged.to_string(),
                    ..course(2, 50, "q-course-a", 1)
                }],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        let renamed: i64 = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = 'Qother#00002'",
                [],
                |row| row.get(0),
            )
            .expect("renamed account");
        assert_eq!(
            observations(&conn, renamed).expect("read"),
            vec![CourseObservation {
                account_username: "Qother#00002".to_string(),
                ..course(2, 50, "q-course-a", 1)
            }]
        );
    }

    fn stubbed(conn: &Connection) -> Vec<i64> {
        let mut statement = conn
            .prepare("SELECT catalog_id FROM cards ORDER BY catalog_id")
            .expect("prepare");
        statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    /// A draft pool not yet played is stored nowhere else: its cards need a
    /// stub row for the card data pass to fetch them.
    #[test]
    fn a_stored_snapshot_stubs_its_cards() {
        let (_dir, mut conn, _) = setup("arena-courses-stubs");
        let batch = WriteBatch {
            arena_course_observations: vec![CourseObservation {
                main: vec![(100_090_001, 2)],
                side: vec![(100_090_002, 1)],
                pool: vec![100_090_001, 100_090_003],
                ..course(1, 10, "q-course-a", 1)
            }],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("commit");
        assert_eq!(stubbed(&conn), vec![100_090_001, 100_090_002, 100_090_003]);

        conn.execute("DELETE FROM cards", []).expect("clear cards");
        let tx = conn.transaction().expect("tx");
        crate::cards::scan_missing(&tx).expect("scan");
        tx.commit().expect("commit scan");
        assert_eq!(
            stubbed(&conn),
            vec![100_090_001, 100_090_002, 100_090_003],
            "the full scan reads stored snapshots"
        );
    }

    #[test]
    fn match_event_names_keep_the_first_name_per_match() {
        let (_dir, mut conn, account_id) = setup("arena-match-events");
        conn.execute(
            "INSERT INTO matches (token, account_id, source, format, match_type, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES ('q-match', ?1, 'log', 'S6EOE', 'limited', 'complete', 'win', 1, 2, 1, 0)",
            [account_id],
        )
        .expect("match");
        let batch = |name: &str| WriteBatch {
            arena_match_events: vec![
                ("q-match".to_string(), name.to_string()),
                ("q-unstored".to_string(), name.to_string()),
            ],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch("Sealed_EOE_20261106")).expect("commit");
        commit(&mut conn, &batch("Other_Event")).expect("commit again");

        assert_eq!(
            match_event_names(&conn, account_id).expect("names"),
            HashMap::from([("q-match".to_string(), "Sealed_EOE_20261106".to_string())])
        );
        assert_eq!(
            match_event_tokens(&conn).expect("tokens"),
            HashSet::from(["q-match".to_string(), "q-unstored".to_string()])
        );
    }
}
