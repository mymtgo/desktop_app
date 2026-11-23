//! The Opponents page (0.x `Opponents\IndexController`): every opponent the
//! account has played, with the record against them, the formats they were
//! met in and when they were last played.
//!
//! v1 differences from 0.x: opponents are counted by `matches.opponent_id`
//! over `listed_matches` (hidden matches left out), complete matches only,
//! imported and manual matches included, and no deck is needed (0.x required
//! a deck version). The account is the current one (`account`); `None`
//! counts every account.
//!
//! Records follow 0.x `MatchRecord::fromTotal`: anything that is neither a
//! win nor a loss (a draw, an unknown outcome) counts as a draw.

use decks::display_format;
use decks::stats::Record;
use rusqlite::{Connection, params};
use std::collections::HashMap;

use crate::StoreError;
use crate::platform::Platform;

/// Opponents per page (0.x `paginate(25)`).
pub const OPPONENTS_PER_PAGE: u32 = 25;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpponentSort {
    /// Most matches first (the default).
    #[default]
    MostPlayed,
    /// Best win rate first.
    WinRateDesc,
    /// Worst win rate first.
    WinRateAsc,
    /// Most recently played first.
    MostRecent,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpponentsFilter {
    /// Text the username contains (case-insensitive, `%` and `_` literal).
    pub search: Option<String>,
    pub sort: OpponentSort,
    /// A format code, as stored on the match.
    pub format: Option<String>,
    /// 1-based; 0 counts as 1.
    pub page: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpponentRow {
    pub opponent_id: i64,
    pub username: String,
    pub record: Record,
    /// Display names of every format met in, sorted.
    pub formats: Vec<String>,
    pub last_played_at_ms: i64,
    /// The archetypes the opponent was seen with, by name (0.x
    /// `archetypes`).
    pub archetypes: Vec<OpponentArchetype>,
}

/// An archetype an opponent was seen with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpponentArchetype {
    pub name: String,
    /// Comma separated colours, `None` when unknown.
    pub color_identity: Option<String>,
}

/// A format the filter offers: its stored code and display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOption {
    pub code: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpponentsPage {
    pub rows: Vec<OpponentRow>,
    /// Opponents over every page.
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    /// Formats of the account's complete matches that have an opponent.
    pub formats: Vec<FormatOption>,
}

/// The account's (`?1`) complete, listed matches that have an opponent:
/// every match of the platform's (`?2`) accounts without one.
const MATCHES: &str = "listed_matches m JOIN opponents o ON o.id = m.opponent_id
WHERE m.state = 'complete' AND (?1 IS NULL OR m.account_id = ?1)
  AND m.account_id IN (SELECT id FROM accounts WHERE platform = ?2)";

/// `%`, `_` and the escape character itself, made literal for `LIKE`.
fn like_pattern(search: &str) -> String {
    let mut out = String::from("%");
    for c in search.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn order_sql(sort: OpponentSort) -> &'static str {
    match sort {
        OpponentSort::MostPlayed => "total DESC, name ASC, o.id ASC",
        OpponentSort::WinRateDesc => {
            "CAST(wins AS REAL) / total DESC, total DESC, name ASC, o.id ASC"
        }
        OpponentSort::WinRateAsc => {
            "CAST(wins AS REAL) / total ASC, total DESC, name ASC, o.id ASC"
        }
        OpponentSort::MostRecent => "last_played DESC, name ASC, o.id ASC",
    }
}

/// The grouped formats read for the page's opponent ids (`?1` is the
/// account, `?2` the platform).
fn formats_sql(id_list: &str) -> String {
    format!(
        "SELECT DISTINCT m.opponent_id, m.format FROM {MATCHES}
  AND m.opponent_id IN ({id_list}) AND m.format IS NOT NULL"
    )
}

pub fn opponents_page(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    filter: &OpponentsFilter,
) -> Result<OpponentsPage, StoreError> {
    let page = filter.page.max(1);
    let offset = (page - 1).saturating_mul(OPPONENTS_PER_PAGE);
    let search = filter
        .search
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(like_pattern);
    let format = filter.format.as_deref().filter(|code| !code.is_empty());
    // An Arena opponent is searched and shown by its display name.
    let conditions = "AND (?3 IS NULL OR COALESCE(o.display_name, o.username) LIKE ?3 ESCAPE '\\')
  AND (?4 IS NULL OR m.format = ?4)";

    let total: u32 = conn
        .prepare_cached(&format!(
            "SELECT COUNT(DISTINCT o.id) FROM {MATCHES}\n  {conditions}"
        ))?
        .query_row(params![account, platform, search, format], |row| row.get(0))?;

    let sql = format!(
        "SELECT o.id, COALESCE(o.display_name, o.username) AS name,
    SUM(m.outcome = 'win') AS wins, SUM(m.outcome = 'loss') AS losses,
    COUNT(*) AS total, MAX(m.started_at) AS last_played
FROM {MATCHES}\n  {conditions}
GROUP BY o.id
ORDER BY {order}
LIMIT ?5 OFFSET ?6",
        order = order_sql(filter.sort)
    );
    let mut rows = Vec::new();
    let mut stmt = conn.prepare_cached(&sql)?;
    let found = stmt.query_map(
        params![
            account,
            platform,
            search,
            format,
            OPPONENTS_PER_PAGE,
            offset
        ],
        |row| {
            let wins: u32 = row.get(2)?;
            let losses: u32 = row.get(3)?;
            let total: u32 = row.get(4)?;
            Ok(OpponentRow {
                opponent_id: row.get(0)?,
                username: row.get(1)?,
                record: Record::from_total(wins, losses, total),
                formats: Vec::new(),
                last_played_at_ms: row.get(5)?,
                archetypes: Vec::new(),
            })
        },
    )?;
    for row in found {
        rows.push(row?);
    }
    drop(stmt);

    // Every format the page's opponents were met in, whatever the format
    // filter says (0.x did the same), in one grouped query. The ids are
    // our own integers, so they go into the text.
    if !rows.is_empty() {
        let ids: Vec<String> = rows.iter().map(|row| row.opponent_id.to_string()).collect();
        let mut formats_stmt = conn.prepare(&formats_sql(&ids.join(", ")))?;
        let found = formats_stmt.query_map(params![account, platform], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut by_opponent: HashMap<i64, Vec<String>> = HashMap::new();
        for pair in found {
            let (opponent_id, code) = pair?;
            let names = by_opponent.entry(opponent_id).or_default();
            let name = display_format(&code);
            if !names.contains(&name) {
                names.push(name);
            }
        }
        for row in &mut rows {
            if let Some(mut names) = by_opponent.remove(&row.opponent_id) {
                names.sort();
                row.formats = names;
            }
        }
    }

    // The opponent seat's archetypes of the opponent's matches, this page
    // only (0.x batch-loaded them per page).
    let mut archetypes_stmt = conn.prepare_cached(
        "SELECT DISTINCT a.name, a.color_identity
         FROM listed_matches m
         JOIN match_archetypes x ON x.match_id = m.id AND x.is_opponent = 1
         JOIN archetypes a ON a.id = x.archetype_id
         WHERE m.opponent_id = ?1
         ORDER BY a.name, a.color_identity",
    )?;
    for row in &mut rows {
        let found = archetypes_stmt.query_map([row.opponent_id], |r| {
            Ok(OpponentArchetype {
                name: r.get(0)?,
                color_identity: r.get(1)?,
            })
        })?;
        for archetype in found {
            row.archetypes.push(archetype?);
        }
    }
    drop(archetypes_stmt);

    let mut options = Vec::new();
    let mut options_stmt = conn.prepare_cached(&format!(
        "SELECT DISTINCT m.format FROM {MATCHES} AND m.format IS NOT NULL"
    ))?;
    for code in options_stmt.query_map(params![account, platform], |r| r.get::<_, String>(0))? {
        let code = code?;
        options.push(FormatOption {
            name: display_format(&code),
            code,
        });
    }
    options.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.code.cmp(&b.code)));

    Ok(OpponentsPage {
        rows,
        total,
        page,
        per_page: OPPONENTS_PER_PAGE,
        formats: options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::hide;
    use fixture_tools::TestDir;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0), (2, 'Qsecond', 0);",
        )
        .expect("accounts");
        (dir, conn)
    }

    fn opponent(conn: &Connection, id: i64, name: &str) {
        conn.execute(
            "INSERT INTO opponents (id, username, first_seen_at) VALUES (?1, ?2, 0)",
            params![id, name],
        )
        .expect("opponent");
    }

    fn play(
        conn: &Connection,
        token: &str,
        account: i64,
        opp: i64,
        format: &str,
        outcome: &str,
        at: i64,
    ) {
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES (?1, ?2, ?3, 'log', ?4, 'complete', ?5, ?6, ?6, 1, 1)",
            params![token, account, opp, format, outcome, at],
        )
        .expect("match");
    }

    fn names(page: &OpponentsPage) -> Vec<&str> {
        page.rows.iter().map(|r| r.username.as_str()).collect()
    }

    fn filter(sort: OpponentSort) -> OpponentsFilter {
        OpponentsFilter {
            sort,
            page: 1,
            ..OpponentsFilter::default()
        }
    }

    /// Qann: 3 matches (W W L) modern + legacy, last at 30. Qbob: 2 (L D),
    /// modern, last at 50. Qcat: 1 (W), modern, last at 10.
    fn seeded(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        opponent(&conn, 1, "Qann");
        opponent(&conn, 2, "Qbob");
        opponent(&conn, 3, "Qcat");
        play(&conn, "a1", 1, 1, "CMODERN", "win", 10);
        play(&conn, "a2", 1, 1, "CLEGACY", "win", 20);
        play(&conn, "a3", 1, 1, "CMODERN", "loss", 30);
        play(&conn, "b1", 1, 2, "CMODERN", "loss", 40);
        play(&conn, "b2", 1, 2, "CMODERN", "draw", 50);
        play(&conn, "c1", 1, 3, "CMODERN", "win", 10);
        (dir, conn)
    }

    #[test]
    fn the_formats_read_seeks_the_opponent_index() {
        let (_dir, conn) = db("opp-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", formats_sql("1, 2, 3")))
            .expect("explain");
        let plan: Vec<String> = stmt
            .query_map(params![1, Platform::Mtgo], |r| r.get::<_, String>(3))
            .expect("rows")
            .map(|r| r.expect("row"))
            .collect();
        assert!(
            plan.iter()
                .any(|line| line.contains("USING INDEX matches_opponent_id")),
            "{plan:?}"
        );
    }

    #[test]
    fn matches_group_by_opponent_with_a_record_formats_and_last_played() {
        let (_dir, conn) = seeded("opp-group");
        let page = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("page");
        assert_eq!(names(&page), ["Qann", "Qbob", "Qcat"]);
        assert_eq!(page.total, 3);
        let ann = &page.rows[0];
        assert_eq!(ann.record, Record::from_counts(2, 1, 0));
        assert_eq!(ann.formats, ["Legacy", "Modern"]);
        assert_eq!(ann.last_played_at_ms, 30);
        // A draw counts in the total and the win rate.
        let bob = &page.rows[1];
        assert_eq!(bob.record, Record::from_counts(0, 1, 1));
        assert_eq!(bob.record.win_rate, 0);
    }

    #[test]
    fn each_opponent_lists_the_archetypes_it_was_seen_with() {
        // 0.x Opponents\IndexController `archetypes`: the distinct opponent
        // seat archetypes of the opponent's matches, by name, with their
        // colours; the player's own seat is never listed.
        let (_dir, conn) = seeded("opp-archetypes");
        conn.execute_batch(
            "INSERT INTO archetypes (id, uuid, name, format, color_identity, source,
                 created_at, updated_at)
             VALUES (50, 'q-burn', 'Qburn', 'modern', 'R', 'downloaded', 0, 0),
                    (51, 'q-tron', 'Qtron', 'modern', NULL, 'downloaded', 0, 0),
                    (52, 'q-mine', 'Qmine', 'modern', 'U', 'manual', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             SELECT id, 1, 51, 1.0 FROM matches WHERE token = 'a1';
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             SELECT id, 1, 50, 1.0 FROM matches WHERE token IN ('a2', 'a3');
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             SELECT id, 0, 52, 1.0 FROM matches WHERE token = 'b1';",
        )
        .expect("archetypes");
        let page = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("page");
        assert_eq!(
            page.rows[0].archetypes,
            vec![
                OpponentArchetype {
                    name: "Qburn".to_string(),
                    color_identity: Some("R".to_string()),
                },
                OpponentArchetype {
                    name: "Qtron".to_string(),
                    color_identity: None,
                },
            ]
        );
        assert!(page.rows[1].archetypes.is_empty(), "the player's seat");
        assert!(page.rows[2].archetypes.is_empty());
    }

    #[test]
    fn a_hidden_match_adds_no_archetype_label() {
        let (_dir, conn) = seeded("opp-archetypes-hidden");
        conn.execute_batch(
            "INSERT INTO archetypes (id, uuid, name, format, color_identity, source,
                 created_at, updated_at)
             VALUES (50, 'q-burn', 'Qburn', 'modern', 'R', 'downloaded', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             SELECT id, 1, 50, 1.0 FROM matches WHERE token = 'a1';",
        )
        .expect("archetypes");
        hide(&conn, "a1");
        let page = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("page");
        assert!(page.rows[0].archetypes.is_empty(), "{:?}", page.rows[0]);
    }

    #[test]
    fn each_sort_orders_with_ties_by_name() {
        let (_dir, conn) = seeded("opp-sorts");
        let by = |sort| {
            let page = opponents_page(&conn, Some(1), Platform::Mtgo, &filter(sort)).expect("page");
            names(&page)
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(by(OpponentSort::MostPlayed), ["Qann", "Qbob", "Qcat"]);
        assert_eq!(by(OpponentSort::WinRateDesc), ["Qcat", "Qann", "Qbob"]);
        assert_eq!(by(OpponentSort::WinRateAsc), ["Qbob", "Qann", "Qcat"]);
        assert_eq!(by(OpponentSort::MostRecent), ["Qbob", "Qann", "Qcat"]);
        // Equal totals and equal last played fall back to the name.
        opponent(&conn, 4, "Qabe");
        play(&conn, "d1", 1, 4, "CMODERN", "win", 10);
        assert_eq!(
            by(OpponentSort::MostPlayed),
            ["Qann", "Qbob", "Qabe", "Qcat"]
        );
        assert_eq!(
            by(OpponentSort::MostRecent),
            ["Qbob", "Qann", "Qabe", "Qcat"]
        );
    }

    #[test]
    fn search_is_a_literal_case_insensitive_contains() {
        let (_dir, conn) = seeded("opp-search");
        opponent(&conn, 5, "Q_100%");
        opponent(&conn, 6, "Qx100y");
        play(&conn, "e1", 1, 5, "CMODERN", "win", 1);
        play(&conn, "e2", 1, 6, "CMODERN", "win", 1);
        let find = |text: &str| {
            let f = OpponentsFilter {
                search: Some(text.to_string()),
                ..filter(OpponentSort::MostPlayed)
            };
            let page = opponents_page(&conn, Some(1), Platform::Mtgo, &f).expect("page");
            names(&page)
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(find("AN"), ["Qann"]);
        assert_eq!(find("_1"), ["Q_100%"]);
        assert_eq!(find("0%"), ["Q_100%"]);
        assert_eq!(find("%"), ["Q_100%"]);
        assert_eq!(find("_"), ["Q_100%"]);
        assert_eq!(find("  "), find(""));
        assert_eq!(find("zzz"), Vec::<String>::new());
    }

    #[test]
    fn the_format_filter_keeps_that_format_and_options_list_the_account_s_formats() {
        let (_dir, conn) = seeded("opp-format");
        let f = OpponentsFilter {
            format: Some("CLEGACY".to_string()),
            ..filter(OpponentSort::MostPlayed)
        };
        let page = opponents_page(&conn, Some(1), Platform::Mtgo, &f).expect("page");
        assert_eq!(names(&page), ["Qann"]);
        assert_eq!(page.rows[0].record.total, 1);
        // The row still lists every format met in.
        assert_eq!(page.rows[0].formats, ["Legacy", "Modern"]);
        assert_eq!(
            page.formats,
            [
                FormatOption {
                    code: "CLEGACY".into(),
                    name: "Legacy".into()
                },
                FormatOption {
                    code: "CMODERN".into(),
                    name: "Modern".into()
                },
            ]
        );
    }

    #[test]
    fn hidden_incomplete_and_other_account_matches_are_excluded() {
        let (_dir, conn) = seeded("opp-excluded");
        hide(&conn, "a3");
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES ('live', 1, 3, 'log', 'in_progress', 'unknown', 99, 99, 1, 1)",
            [],
        )
        .expect("live");
        play(&conn, "o1", 2, 3, "CVINTAGE", "loss", 60);
        opponent(&conn, 7, "Qonly");
        play(&conn, "o2", 2, 7, "CVINTAGE", "win", 61);
        let page = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("page");
        assert_eq!(page.rows[0].record, Record::from_counts(2, 0, 0));
        let cat = page
            .rows
            .iter()
            .find(|r| r.username == "Qcat")
            .expect("cat");
        assert_eq!(cat.record, Record::from_counts(1, 0, 0));
        assert_eq!(cat.last_played_at_ms, 10);
        assert_eq!(cat.formats, ["Modern"]);
        assert!(!names(&page).contains(&"Qonly"));
        assert_eq!(page.formats.len(), 2);
        let second = opponents_page(
            &conn,
            Some(2),
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("2");
        assert_eq!(names(&second), ["Qcat", "Qonly"]);
        let every = opponents_page(
            &conn,
            None,
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("all");
        assert_eq!(every.total, 4);
    }

    #[test]
    fn the_opponents_page_reads_one_platform() {
        let (_dir, conn) = seeded("opp-arena");
        opponent(&conn, 8, "arena:Qarena-opponent");
        play(&conn, "r1", 1, 8, "CPIONEER", "win", 70);
        crate::test_support::move_match_to_arena(&conn, "r1");
        let every = opponents_page(
            &conn,
            None,
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("all");
        assert_eq!(names(&every), ["Qann", "Qbob", "Qcat"]);
        assert_eq!(every.total, 3);
        let codes: Vec<&str> = every.formats.iter().map(|f| f.code.as_str()).collect();
        assert!(!codes.contains(&"CPIONEER"), "{codes:?}");

        let arena = opponents_page(
            &conn,
            None,
            Platform::Arena,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("arena");
        assert_eq!(names(&arena), ["arena:Qarena-opponent"]);
        assert_eq!(arena.total, 1);
        let codes: Vec<&str> = arena.formats.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, ["CPIONEER"]);
        assert_eq!(arena.rows[0].formats, ["Pioneer"]);
    }

    #[test]
    fn opponent_search_matches_the_display_name() {
        let (_dir, conn) = seeded("opp-display-name");
        opponent(&conn, 8, "arena:u1");
        crate::test_support::name_arena_opponent(&conn, "arena:u1", "Opponent");
        play(&conn, "r1", 1, 8, "CPIONEER", "win", 70);
        crate::test_support::move_match_to_arena(&conn, "r1");
        let search = |text: &str| OpponentsFilter {
            search: Some(text.to_string()),
            ..filter(OpponentSort::MostPlayed)
        };
        let found = opponents_page(&conn, None, Platform::Arena, &search("oppo")).expect("found");
        assert_eq!(names(&found), ["Opponent"]);
        let by_key = opponents_page(&conn, None, Platform::Arena, &search("u1")).expect("key");
        assert!(by_key.rows.is_empty(), "the registry key is not a name");
    }

    #[test]
    fn pages_hold_25_with_no_repeats_and_an_overshoot_is_empty() {
        let (_dir, conn) = db("opp-pages");
        for i in 0..30 {
            opponent(&conn, i + 1, &format!("Qp{i:02}"));
            play(&conn, &format!("p{i}"), 1, i + 1, "CMODERN", "win", 1);
        }
        let one = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &filter(OpponentSort::MostPlayed),
        )
        .expect("1");
        let two = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &OpponentsFilter {
                page: 2,
                ..filter(OpponentSort::MostPlayed)
            },
        )
        .expect("2");
        assert_eq!((one.total, one.rows.len(), two.rows.len()), (30, 25, 5));
        assert_eq!(one.rows[0].username, "Qp00");
        assert_eq!(two.rows[0].username, "Qp25");
        let past = opponents_page(
            &conn,
            Some(1),
            Platform::Mtgo,
            &OpponentsFilter {
                page: 9,
                ..filter(OpponentSort::MostPlayed)
            },
        )
        .expect("9");
        assert!(past.rows.is_empty());
        assert_eq!(past.total, 30);
    }

    #[test]
    fn no_matches_is_an_empty_page() {
        let (_dir, conn) = db("opp-empty");
        let page = opponents_page(&conn, Some(1), Platform::Mtgo, &OpponentsFilter::default())
            .expect("page");
        assert!(page.rows.is_empty());
        assert_eq!((page.total, page.page), (0, 1));
        assert!(page.formats.is_empty());
    }
}
#[cfg(test)]
mod speed {
    use super::*;
    use crate::open_writer;
    use fixture_tools::TestDir;
    use std::time::Instant;

    /// `cargo test --release -p store --lib opponents::speed -- --ignored --nocapture`
    #[test]
    #[ignore = "speed measurement, run in release"]
    fn opponents_page_over_3600_matches() {
        let dir = TestDir::new("opp-speed");
        let conn = open_writer(&dir.join("m.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 400)
             INSERT INTO opponents (id, username, first_seen_at) SELECT i, 'Qopp' || i, 0 FROM n;
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 3600)
             INSERT INTO matches (token, account_id, opponent_id, source, format, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             SELECT 't' || i, 1, 1 + (i % 400),
                 'log', CASE i % 3 WHEN 0 THEN 'CMODERN' WHEN 1 THEN 'CLEGACY' ELSE 'CPAUPER' END,
                 'complete', CASE i % 2 WHEN 0 THEN 'win' ELSE 'loss' END, i, i, 1, 1 FROM n;",
        )
        .expect("seed");
        let filter = OpponentsFilter {
            page: 1,
            ..OpponentsFilter::default()
        };
        let mut best = f64::MAX;
        for _ in 0..20 {
            let start = Instant::now();
            let page = opponents_page(&conn, Some(1), Platform::Mtgo, &filter).expect("page");
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(page.rows.len(), 25);
        }
        println!("opponents_page best of 20: {best:.2} ms");
    }
}
