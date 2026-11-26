use super::*;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use fixture_tools::{TestDir, load_fixture};
use mtgo_log::Env;
use store::imported_detail::{DetailCard, DetailLogEntry, ImportedDetail, ImportedGameDetail};
use store::params;
use tracker::{Engine, TrackerConfig};

use crate::commands::tests::ready_state;
use crate::leagues::{ClassificationView, MarkView, RunSource, RunStateView, TimeOfDayView};

/// 2026-07-01T10:00:00Z.
const T0: i64 = 1_782_900_000_000;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const UTC: Tz = chrono_tz::UTC;
/// Now, for every read: an hour after T0.
const NOW: i64 = T0 + HOUR;

// ---- Reads after a sync ---------------------------------------------------------
//
// The pages read what the league sync stored. Each read below syncs first, at
// its `now` and in its zone, as the tracker does before the pages read (an
// open draft idle for 30 minutes by then is abandoned), then reads.

/// A league sync of every owner at `now_ms` in `tz` (none on a read-only
/// connection: the tracker syncs then).
fn sync(conn: &Connection, now_ms: i64, tz: Tz) {
    if conn.is_readonly("main").expect("read-only?") {
        return;
    }
    let tx = conn.unchecked_transaction().expect("transaction");
    // Take the write lock first: a running tracker may write meanwhile.
    tx.execute("UPDATE accounts SET id = id WHERE 0", [])
        .expect("write lock");
    store::league_sync::sync_every_owner(&tx, tz, now_ms).expect("league sync");
    tx.commit().expect("commit");
}

fn index(
    conn: &Connection,
    query: &LimitedIndexQuery,
    now_ms: i64,
    tz: Tz,
) -> Result<LimitedIndexView, CommandError> {
    sync(conn, now_ms, tz);
    super::index(conn, query, now_ms, tz)
}

fn run_colours(
    conn: &Connection,
    key: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<Vec<LimitedMatchColoursView>>, CommandError> {
    sync(conn, now_ms, tz);
    super::run_colours(conn, key)
}

fn event(
    conn: &Connection,
    key: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<LimitedEventView>, CommandError> {
    sync(conn, now_ms, tz);
    super::event(conn, key, tz)
}

fn draft_page(
    conn: &Connection,
    key: &str,
    pick: Option<u32>,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<LimitedDraftView>, CommandError> {
    sync(conn, now_ms, tz);
    super::draft_page(conn, key, pick)
}

fn cross_drafts(
    conn: &Connection,
    key: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<CrossDraftsView>, CommandError> {
    sync(conn, now_ms, tz);
    super::cross_drafts(conn, key)
}

fn deck_page(
    conn: &Connection,
    key: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<LimitedDeckView>, CommandError> {
    sync(conn, now_ms, tz);
    super::deck_page(conn, key)
}

fn cards_page(
    conn: &Connection,
    key: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<LimitedCardsView>, CommandError> {
    sync(conn, now_ms, tz);
    super::cards_page(conn, key)
}

fn matches_page(
    conn: &Connection,
    key: &str,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<LimitedMatchesView>, CommandError> {
    sync(conn, now_ms, tz);
    super::matches_page(conn, key, tz)
}

fn match_page(
    conn: &Connection,
    key: &str,
    token: &str,
    dat: &[game_detail::GameLogEntry],
    now_ms: i64,
    tz: Tz,
) -> Result<Option<MatchDetail>, CommandError> {
    sync(conn, now_ms, tz);
    super::match_page(conn, key, token, dat)
}

fn notes_window(
    conn: &Connection,
    now_ms: i64,
    tz: Tz,
) -> Result<Option<DraftNotesView>, CommandError> {
    sync(conn, now_ms, tz);
    super::notes_window(conn, now_ms)
}

fn gather(conn: &Connection, now_ms: i64, tz: Tz) -> Result<Gathered, StoreError> {
    sync(conn, now_ms, tz);
    super::gather(conn)
}

fn db(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    conn.execute(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
        [],
    )
    .expect("account");
    (dir, conn)
}

/// A 0.x run of the local account (`0x:{id}`).
fn run(
    conn: &Connection,
    id: i64,
    kind: &str,
    set_code: Option<&str>,
    state: &str,
    started_at: i64,
) {
    let format = match kind {
        "sealed" => "S6QSL",
        "constructed" => "CMODERN",
        _ => "Limited",
    };
    let seed = store::league_sync::ImportedSeed {
        token: format!("q-league-{id}"),
        event_id: None,
        name: format!("Q League {id}"),
        format: Some(format.to_string()),
        state: state.to_string(),
        started_at,
        joined_at: None,
        completed_at: None,
        dropped_at: None,
        deck_version_id: None,
        kind: kind.to_string(),
        set_code: set_code.map(str::to_string),
        mtgo_course_id: None,
        legacy_id: None,
    };
    // As the 0.x import stores it: row `1000 + id`, the seed's values.
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name, format,
             set_code, started_at, created_at, imported_from, imported_seed)
         VALUES (1000 + ?1, '0x:' || ?1, 1, 'imported', ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?1, ?9)",
        params![
            id,
            seed.kind,
            seed.state,
            seed.token,
            seed.name,
            seed.format,
            seed.set_code,
            seed.started_at,
            serde_json::to_string(&seed).expect("seed")
        ],
    )
    .expect("run");
}

fn event_id(conn: &Connection, run_id: i64, event_id: i64) {
    conn.execute(
        "UPDATE leagues SET event_id = ?2,
             imported_seed = json_set(imported_seed, '$.event_id', ?2)
         WHERE imported_from = ?1",
        params![run_id, event_id],
    )
    .expect("event id");
    // The import gives the run's drafts its event id.
    conn.execute(
        "UPDATE drafts SET league_event_id = ?2 WHERE imported_league_from = ?1",
        params![run_id, event_id],
    )
    .expect("the drafts' event id");
}

/// A complete 0.x match of run `run_id` against `Qopponent`, one game per
/// result in `games`.
#[allow(clippy::too_many_arguments)]
fn played(
    conn: &Connection,
    run_id: i64,
    token: &str,
    format: &str,
    outcome: &str,
    started_at: i64,
    ended_at: Option<i64>,
    games: &[&str],
) {
    conn.execute(
        "INSERT INTO opponents (username, first_seen_at) VALUES ('Qopponent', 0)
         ON CONFLICT (username) DO NOTHING",
        [],
    )
    .expect("opponent");
    let id: i64 = conn
        .query_row(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, state, outcome,
                 started_at, ended_at, last_activity_at, revision, derived_version)
             VALUES (?1, 1, (SELECT id FROM opponents WHERE username = 'Qopponent'), 'imported',
                 ?2, 'complete', ?3, ?4, ?5, ?4, 1, 0)
             RETURNING id",
            params![token, format, outcome, started_at, ended_at],
            |row| row.get(0),
        )
        .expect("match");
    for (index, result) in games.iter().enumerate() {
        let number = index as i64 + 1;
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play, started_at,
                 ended_at)
             VALUES (?1, ?2, ?3, 'win', ?4, ?5, ?6)",
            params![
                id,
                number,
                result,
                number == 1,
                started_at + number * MINUTE,
                started_at + number * 2 * MINUTE
            ],
        )
        .expect("game");
    }
    if run_id > 0 {
        conn.execute(
            "UPDATE matches SET league_id = 1000 + ?2, league_linked = 1 WHERE token = ?1",
            params![token, run_id],
        )
        .expect("member");
    }
}

/// A 0.x draft, as the import stores it (its league's event id and course id
/// on it); `run_id` 0 for none.
fn draft(conn: &Connection, id: i64, run_id: i64, state: &str, started_at: Option<i64>) {
    conn.execute(
        "INSERT INTO drafts (id, draft_token, account_id, source, league_event_id,
             mtgo_course_id, seat_count, seat_index, state, pack_size, picks_expected,
             started_at, imported_from, imported_league_from)
         VALUES (?1, 'q-draft-' || ?1, 1, 'imported',
             (SELECT json_extract(imported_seed, '$.event_id') FROM leagues
              WHERE imported_from = ?2),
             (SELECT json_extract(imported_seed, '$.mtgo_course_id') FROM leagues
              WHERE imported_from = ?2),
             8, 5, ?3, 14, 42, ?4, ?1, ?2)",
        params![id, (run_id > 0).then_some(run_id), state, started_at],
    )
    .expect("draft");
}

/// A pick of draft `draft_id`.
struct PickSeed {
    ordinal: u32,
    available: Vec<u32>,
    picked: Option<u32>,
    shown_at: Option<i64>,
    picked_at: Option<i64>,
    deadline_at: Option<i64>,
    reservations: usize,
}

fn pick_seed(ordinal: u32, picked: Option<u32>) -> PickSeed {
    PickSeed {
        ordinal,
        available: picked.into_iter().collect(),
        picked,
        shown_at: None,
        picked_at: None,
        deadline_at: None,
        reservations: 0,
    }
}

fn pick(conn: &Connection, draft_id: i64, seed: &PickSeed) {
    let pack = (seed.ordinal - 1) / 14 + 1;
    let number = (seed.ordinal - 1) % 14 + 1;
    let available = serde_json::to_string(&seed.available).expect("available as JSON");
    let mut reservations = Vec::new();
    for index in 0..seed.reservations {
        reservations.push(
            serde_json::json!({ "catalog_id": index + 1, "at_ms": seed.shown_at.unwrap_or(0) }),
        );
    }
    conn.execute(
        "INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number, pack_id,
             cards_available, picked_catalog_id, reservations, shown_at, deadline_at, picked_at)
         VALUES (?1, ?2, ?3, ?4, 500, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            draft_id,
            seed.ordinal,
            pack,
            number,
            available,
            seed.picked,
            serde_json::Value::Array(reservations).to_string(),
            seed.shown_at,
            seed.deadline_at,
            seed.picked_at
        ],
    )
    .expect("pick");
}

/// The deck registered for `match_token` of run `run_id`, as the import
/// stores it (the match's `imported` snapshot):
/// `(catalog id, quantity, sideboard)`.
fn registered(
    conn: &Connection,
    _run_id: i64,
    match_token: &str,
    cards: &[(u32, u32, bool)],
    at: i64,
) {
    let rows: Vec<serde_json::Value> = cards
        .iter()
        .map(|(id, quantity, side)| {
            serde_json::json!({ "catalog_id": id, "quantity": quantity, "sideboard": side })
        })
        .collect();
    conn.execute(
        "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
         SELECT id, 'imported', ?2, '', ?3 FROM matches WHERE token = ?1",
        params![match_token, serde_json::Value::Array(rows).to_string(), at],
    )
    .expect("registered");
}

fn cards(conn: &Connection, sql_values: &str) {
    conn.execute_batch(&format!(
        "INSERT INTO cards (catalog_id, name, type, rarity, colors, color_identity, set_code,
             set_name, oracle_id, art_crop) VALUES {sql_values};"
    ))
    .expect("cards");
}

fn index_query(
    set: Option<&str>,
    kind: Option<&str>,
    timeframe: Option<&str>,
) -> LimitedIndexQuery {
    LimitedIndexQuery {
        set: set.map(str::to_string),
        kind: kind.map(str::to_string),
        timeframe: timeframe.map(str::to_string),
    }
}

fn the_index(conn: &Connection, query: &LimitedIndexQuery) -> LimitedIndexView {
    index(conn, query, NOW, UTC).expect("index")
}

fn titles(view: &LimitedIndexView) -> Vec<String> {
    view.rows.iter().map(|row| row.title.clone()).collect()
}

// ---- Index (0.x IndexControllerTest, BuildLimitedIndexTest) ------------------

#[test]
fn the_index_lists_limited_runs_with_kpis_sets_and_filters() {
    let (_dir, conn) = db("limited-index");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    run(&conn, 2, "constructed", None, "complete", T0);

    let view = the_index(&conn, &index_query(None, None, None));
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].set_code.as_deref(), Some("QHB"));
    assert_eq!(view.rows[0].run_key.as_deref(), Some("0x:1"));
    assert_eq!(view.kpis.events, 1);
    assert_eq!(set_codes_of(&view.sets), vec!["QHB"]);
    assert_eq!(view.filters.timeframe, "alltime");
}

#[test]
fn a_cube_run_is_named_by_its_format_not_a_guessed_set() {
    let (_dir, conn) = db("limited-cube");
    // 0.x guessed the set from the picks: a cube's span every era.
    run(&conn, 1, "draft", Some("PRM"), "complete", T0);
    draft(&conn, 1, 1, "finished", Some(T0));
    pick(&conn, 1, &pick_seed(1, Some(154000)));
    cards(
        &conn,
        "(154000, 'Qlotus', 'Artifact', 'rare', '', '', 'PRM', 'Q Promos', 'o-lotus', 'https://img/lotus.jpg')",
    );
    played(
        &conn,
        1,
        "q-m1",
        "DC03C03C03",
        "win",
        T0 + HOUR,
        None,
        &["win"],
    );
    conn.execute(
        "UPDATE matches SET game_structure = 'Vintage Cube x3' WHERE token = 'q-m1'",
        [],
    )
    .expect("game structure");

    let view = the_index(&conn, &index_query(None, None, None));
    assert_eq!(titles(&view), vec!["Vintage Cube Draft"]);
    assert_eq!(view.rows[0].set_code.as_deref(), Some("C03"));
    assert_eq!(view.sets[0].code, "C03");
    assert_eq!(view.sets[0].name, "Vintage Cube");
    assert_eq!(view.kpis.most_drafted_set.as_deref(), Some("Vintage Cube"));

    let shown = event(&conn, "0x:1", NOW, UTC)
        .expect("event")
        .expect("found");
    assert_eq!(shown.set_label.as_deref(), Some("Vintage Cube"));
    assert_eq!(shown.set_name.as_deref(), Some("Vintage Cube"));
    assert!(
        shown.title.starts_with("Vintage Cube Draft"),
        "{}",
        shown.title
    );
}

#[test]
fn the_index_applies_the_set_kind_and_timeframe_filters() {
    let (_dir, conn) = db("limited-index-filters");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    run(&conn, 2, "sealed", Some("QMS"), "active", T0);
    run(&conn, 3, "sealed", Some("QMS"), "complete", T0 - 30 * DAY);

    let view = the_index(
        &conn,
        &index_query(Some("QMS"), Some("sealed"), Some("week")),
    );
    assert_eq!(view.rows.len(), 1);
    assert_eq!(view.rows[0].kind, "sealed");
    assert_eq!(view.rows[0].run_key.as_deref(), Some("0x:2"));
    assert_eq!(view.filters.set.as_deref(), Some("QMS"));
    assert_eq!(view.filters.kind.as_deref(), Some("sealed"));
    assert_eq!(view.filters.timeframe, "week");
    // An unknown kind reads as none.
    let any = the_index(&conn, &index_query(None, Some("cube"), None));
    assert_eq!(any.rows.len(), 3);
    assert_eq!(any.filters.kind, None);
}

/// Run 1: a complete QHB draft a day ago with 42 timed picks (shown, then
/// picked 12 s later; every fifth pick with two reservations), won and lost
/// once, with a registered deck.
fn seeded_index(conn: &Connection) {
    let start = T0 - DAY;
    run(conn, 1, "draft", Some("QHB"), "complete", start);
    draft(conn, 1, 1, "finished", Some(start));
    for ordinal in 1..=42 {
        let shown = start + i64::from(ordinal) * 30 * SECOND;
        pick(
            conn,
            1,
            &PickSeed {
                ordinal,
                available: vec![1000 + ordinal],
                picked: Some(1000 + ordinal),
                shown_at: Some(shown),
                picked_at: Some(shown + 12 * SECOND),
                deadline_at: Some(shown + 40 * SECOND),
                reservations: if ordinal % 5 == 0 { 2 } else { 0 },
            },
        );
    }
    played(
        conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        start + HOUR,
        Some(start + HOUR + 20 * MINUTE),
        &["win", "win"],
    );
    played(
        conn,
        1,
        "q-m2",
        "DQHBQHBQHB",
        "loss",
        start + 2 * HOUR,
        Some(start + 2 * HOUR + 30 * MINUTE),
        &["loss", "win", "loss"],
    );
    registered(conn, 1, "q-m1", &[(1001, 1, false)], start + HOUR);
}

#[test]
fn a_run_row_has_its_record_picks_state_and_pick_time() {
    let (_dir, conn) = db("limited-index-row");
    run(&conn, 9, "constructed", None, "complete", T0);
    seeded_index(&conn);

    let view = the_index(&conn, &index_query(None, None, None));
    assert_eq!(titles(&view), vec!["QHB Draft".to_string()]);
    let row = &view.rows[0];
    assert_eq!((row.wins, row.losses), (1, 1));
    assert_eq!(row.picks_made, 42);
    assert_eq!(row.picks_expected, 42);
    assert_eq!(row.state, "Complete");
    assert_eq!(row.state_variant, "success");
    assert!(row.deck_registered);
    assert_eq!(row.version_count, 1);
    assert_eq!(row.avg_pick_seconds, Some(12));
    assert_eq!(row.draft_token.as_deref(), Some("q-draft-1"));
    assert_eq!(
        row.results,
        vec![Some(MarkView::W), Some(MarkView::L), None]
    );
    assert_eq!(
        row.opponents,
        vec!["Qopponent".to_string(), "Qopponent".to_string()]
    );
    assert_eq!((row.game_wins, row.game_losses), (3, 2));
    assert!(row.linked);
    assert_eq!(row.note, None);
    assert_eq!(row.packs, None);
    // The shared league run header's numbers, as the run card has them.
    assert_eq!(row.classification, ClassificationView::Finish);
    assert_eq!(row.live_round, None);
    assert_eq!(row.avg_match_seconds, Some(25 * 60));
    assert_eq!(row.time_of_day, Some(TimeOfDayView::Morning));
    // The run's match rows, so the row can expand into them.
    let results: Vec<MarkView> = row.matches.iter().map(|played| played.result).collect();
    assert_eq!(results, vec![MarkView::W, MarkView::L]);
    assert_eq!(row.matches[0].duration_seconds, Some(20 * 60));
    assert_eq!(set_codes_of(&view.sets), vec!["QHB"]);
}

/// A finished draft of no league (`q-draft-1`), then the three rounds of
/// Swiss Draft 19990050 (`DFRAFRAFRA`): round 1 paired 12 minutes after the
/// draft ends, won, lost, won. Returns the draft's start.
fn swiss_draft(conn: &Connection) -> i64 {
    let start = T0 - 5 * HOUR;
    draft(conn, 1, 0, "finished", Some(start));
    conn.execute(
        "UPDATE drafts SET ended_at = ?1 WHERE id = 1",
        [start + 25 * MINUTE],
    )
    .expect("ended");
    for (round, outcome, games) in [
        (1, "win", ["win", "win"].as_slice()),
        (2, "loss", ["loss", "loss"].as_slice()),
        (3, "win", ["win", "loss", "win"].as_slice()),
    ] {
        let token = format!("q-swiss-{round}");
        let at = start + 37 * MINUTE + (round - 1) * HOUR;
        played(
            conn,
            0,
            &token,
            "DFRAFRAFRA",
            outcome,
            at,
            Some(at + 20 * MINUTE),
            games,
        );
        conn.execute(
            "UPDATE matches SET tournament_event_id = 19990050, tournament_round = ?2,
                 game_structure = 'FRAx3'
             WHERE token = ?1",
            params![token, round],
        )
        .expect("a tournament round");
    }
    start
}

#[test]
fn a_swiss_draft_is_a_linked_row_named_for_its_event() {
    let (_dir, conn) = db("limited-index-swiss");
    let start = swiss_draft(&conn);
    // Named for its event: the log's name, else the tournament row's.
    gather(&conn, NOW, UTC).expect("sync");
    conn.execute(
        "UPDATE tournaments SET name = 'Q Fracture Swiss Draft', name_synthesized = 0",
        [],
    )
    .expect("named");

    let view = the_index(&conn, &index_query(None, None, None));
    assert_eq!(view.rows.len(), 1);
    let row = &view.rows[0];
    assert!(row.linked);
    assert!(!row.in_league, "no league row to edit");
    assert_eq!(row.title, "FRA Swiss Draft");
    assert_eq!(row.run_key.as_deref(), Some("tournament:19990050"));
    assert_eq!(row.draft_token.as_deref(), Some("q-draft-1"));
    assert_eq!(row.set_code.as_deref(), Some("FRA"));
    assert_eq!(row.state, "Complete");
    assert_eq!((row.wins, row.losses), (2, 1));
    assert_eq!(
        row.results,
        vec![Some(MarkView::W), Some(MarkView::L), Some(MarkView::W)]
    );
    assert_eq!(row.matches.len(), 3);
    assert_eq!(row.started_at_ms, Some(start));
    assert_eq!(row.note, None);
    assert_eq!(view.kpis.unlinked, 0);
    assert_eq!(view.kpis.events, 1);

    // Its pages read it by its key.
    let matches = matches_page(&conn, "tournament:19990050", NOW, UTC)
        .expect("matches")
        .expect("found");
    assert_eq!(matches.matches.len(), 3);

    // The Limited pages only: not a league run, not on the home dashboard.
    let leagues = crate::leagues::gather(&conn, &[1]).expect("leagues");
    assert!(leagues.runs.is_empty());
    let data = gather(&conn, NOW, UTC).expect("gather");
    assert!(
        super::dashboard_runs(&conn, &data, UTC)
            .expect("runs")
            .is_empty()
    );
}

#[test]
fn an_unfinished_tournament_draft_is_active_until_another_draft_follows_it() {
    let (_dir, conn) = db("limited-index-swiss-dropped");
    swiss_draft(&conn);
    // Round 1 won: an event of unknown kind plays on after a win.
    conn.execute(
        "DELETE FROM matches WHERE token IN ('q-swiss-2', 'q-swiss-3')",
        [],
    )
    .expect("one round");
    let row = the_index(&conn, &index_query(None, None, None)).rows[0].clone();
    assert_eq!(
        (row.title.as_str(), row.state.as_str()),
        ("FRA Tournament Draft", "Active")
    );

    draft(&conn, 2, 0, "finished", Some(NOW - 10 * MINUTE));
    let view = the_index(&conn, &index_query(None, None, None));
    let swiss = view
        .rows
        .iter()
        .find(|row| row.run_key.as_deref() == Some("tournament:19990050"))
        .expect("the Swiss Draft");
    assert_eq!(swiss.state, "Dropped");
}

#[test]
fn an_unlinked_draft_is_a_league_unknown_row() {
    let (_dir, conn) = db("limited-index-unlinked");
    draft(&conn, 1, 0, "picking", Some(NOW - 10 * MINUTE));
    for ordinal in 1..=3 {
        pick(&conn, 1, &pick_seed(ordinal, Some(5)));
    }

    let view = the_index(&conn, &index_query(None, None, None));
    assert_eq!(view.rows.len(), 1);
    let row = &view.rows[0];
    assert!(!row.linked);
    assert_eq!(row.title, "Draft (league unknown)");
    assert_eq!(row.state, "Unlinked");
    assert_eq!(row.state_variant, "warning");
    assert_eq!(
        row.note.as_deref(),
        Some("Will link when a match or pool grant arrives")
    );
    assert_eq!(row.picks_made, 3);
    assert_eq!(row.results, vec![None, None, None]);
    assert!(row.matches.is_empty());
    assert_eq!(row.run_key, None);
    assert_eq!(row.classification, ClassificationView::Empty);
    assert_eq!(row.live_round, None);
    assert_eq!(row.avg_match_seconds, None);
    assert_eq!(row.time_of_day, None);
    assert_eq!(view.kpis.unlinked, 1);
    assert_eq!(view.kpis.events, 1);

    // Sealed shows no drafts.
    let sealed = the_index(&conn, &index_query(None, Some("sealed"), None));
    assert!(sealed.rows.is_empty());
    // Long abandoned.
    let later = index(&conn, &index_query(None, None, None), NOW + DAY, UTC).expect("later");
    assert_eq!(later.rows[0].state, "Draft abandoned");
}

#[test]
fn the_kpis_cover_the_filtered_runs() {
    let (_dir, conn) = db("limited-index-kpis");
    seeded_index(&conn);
    run(&conn, 2, "sealed", Some("QMS"), "complete", T0 - 2 * DAY);
    played(
        &conn,
        2,
        "q-s1",
        "S6QMS",
        "loss",
        T0 - 2 * DAY,
        Some(T0 - 2 * DAY + HOUR),
        &["loss", "loss"],
    );

    let all = the_index(&conn, &index_query(None, None, None));
    assert_eq!(all.kpis.events, 2);
    assert_eq!(all.kpis.drafts, 1);
    assert_eq!(all.kpis.match_win_pct, Some(33));
    assert_eq!((all.kpis.match_wins, all.kpis.match_losses), (1, 2));
    assert_eq!(all.kpis.most_drafted_set.as_deref(), Some("QHB"));
    assert_eq!(all.kpis.avg_pick_seconds, Some(12));
    // 8 of 42 picks with two reservations.
    assert_eq!(all.kpis.indecision_pct, Some(19));
    assert_eq!(set_codes_of(&all.sets), vec!["QHB", "QMS"]);
    assert_eq!(
        titles(&all),
        vec!["QHB Draft".to_string(), "QMS Sealed".to_string()]
    );

    let qhb = the_index(&conn, &index_query(Some("QHB"), None, None));
    assert_eq!(titles(&qhb), vec!["QHB Draft".to_string()]);
    let sealed = the_index(&conn, &index_query(None, Some("sealed"), None));
    assert_eq!(titles(&sealed), vec!["QMS Sealed".to_string()]);
    assert_eq!(sealed.rows[0].packs, Some(6));
}

#[test]
fn states_map_to_0x_badges() {
    let (_dir, conn) = db("limited-index-badges");
    run(&conn, 1, "draft", Some("QHB"), "partial", T0 - 3 * HOUR);
    run(&conn, 2, "draft", Some("QHB"), "active", T0 - 2 * HOUR);
    run(&conn, 3, "draft", Some("QHB"), "active", T0 - HOUR);
    draft(&conn, 3, 3, "abandoned", Some(T0 - HOUR));
    run(&conn, 4, "draft", Some("QHB"), "complete", T0);

    let view = the_index(&conn, &index_query(None, None, None));
    let badges: Vec<(Option<String>, String, String)> = view
        .rows
        .iter()
        .map(|row| {
            (
                row.run_key.clone(),
                row.state.clone(),
                row.state_variant.clone(),
            )
        })
        .collect();
    assert_eq!(
        badges,
        vec![
            (
                Some("0x:4".to_string()),
                "No matches".to_string(),
                "warning".to_string()
            ),
            (
                Some("0x:3".to_string()),
                "Draft abandoned".to_string(),
                "warning".to_string()
            ),
            (
                Some("0x:2".to_string()),
                "Active".to_string(),
                "default".to_string()
            ),
            (
                Some("0x:1".to_string()),
                "Dropped".to_string(),
                "destructive".to_string()
            ),
        ]
    );
    assert_eq!(
        view.rows[0].note.as_deref(),
        Some("Draft finished, league ended without play")
    );
    // An active run is live, playing its first round.
    assert_eq!(view.rows[2].classification, ClassificationView::Live);
    assert_eq!(view.rows[2].live_round, Some(1));
    // A partial run is shown as dropped, but classified as the Leagues page does.
    assert_eq!(view.rows[3].classification, ClassificationView::Finish);
}

// ---- Event (0.x GetLimitedEventSharedPropsTest) -------------------------------

#[test]
fn the_event_has_0x_sidebar_props() {
    let (_dir, conn) = db("limited-event");
    let started = Utc
        .with_ymd_and_hms(2026, 8, 22, 11, 0, 12)
        .single()
        .expect("time")
        .timestamp_millis();
    run(&conn, 1, "draft", Some("QHB"), "complete", started);
    event_id(&conn, 1, 11039);
    draft(&conn, 1, 1, "finished", Some(started));
    pick(&conn, 1, &pick_seed(1, Some(154000)));
    pick(&conn, 1, &pick_seed(2, Some(154001)));
    cards(
        &conn,
        "(154000, 'Qbard', 'Creature', 'uncommon', 'W', 'W', 'QHB', 'Q Hobbit', 'o-bard', 'https://img/bard.jpg')",
    );
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        started + HOUR,
        None,
        &["win"],
    );
    played(
        &conn,
        1,
        "q-m2",
        "DQHBQHBQHB",
        "loss",
        started + 2 * HOUR,
        None,
        &["loss"],
    );

    let event = event(&conn, "0x:1", NOW, UTC)
        .expect("event")
        .expect("found");
    assert_eq!(event.title, "QHB Draft · 22 Aug");
    assert_eq!(event.subtitle, "Q Hobbit · League 11039");
    assert_eq!(event.set_name.as_deref(), Some("Q Hobbit"));
    assert_eq!((event.wins, event.losses), (1, 1));
    assert_eq!((event.picks_made, event.picks_expected), (2, 42));
    assert_eq!(event.state, "Complete");
    assert_eq!(event.cover_art.as_deref(), Some("https://img/bard.jpg"));
    assert_eq!(event.seat_index, Some(5));
    assert_eq!(event.seat_count, 8);
    assert!(!event.deck_registered);
    assert_eq!(event.draft_state, "finished");
    assert_eq!(
        event.started_at_human.as_deref(),
        Some("22 Aug 2026 · 11:00")
    );
    let pages: Vec<LimitedPage> = event.nav.iter().map(|item| item.page).collect();
    assert_eq!(
        pages,
        vec![
            LimitedPage::Draft,
            LimitedPage::Deck,
            LimitedPage::Matches,
            LimitedPage::Cards
        ]
    );
    assert_eq!(event.nav[0].label, "Draft");
}

#[test]
fn the_cover_is_the_first_rare_or_mythic_pick() {
    let (_dir, conn) = db("limited-event-cover");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    draft(&conn, 1, 1, "finished", Some(T0));
    pick(&conn, 1, &pick_seed(1, Some(154030)));
    pick(&conn, 1, &pick_seed(2, Some(154010)));
    pick(&conn, 1, &pick_seed(5, Some(154020)));
    cards(
        &conn,
        "(154030, 'Qcommon', 'Creature', 'common', 'G', 'G', 'QHB', 'Q Hobbit', 'o-c', 'https://img/common.jpg'),
         (154010, 'Qgandalf', 'Creature', 'mythic', 'W', 'W', 'QHB', 'Q Hobbit', 'o-g', 'https://img/gandalf.jpg'),
         (154020, 'Qsmaug', 'Creature', 'rare', 'R', 'R', 'QHB', 'Q Hobbit', 'o-s', 'https://img/smaug.jpg')",
    );

    let event = event(&conn, "0x:1", NOW, UTC)
        .expect("event")
        .expect("found");
    assert_eq!(event.cover_art.as_deref(), Some("https://img/gandalf.jpg"));
}

#[test]
fn an_event_without_a_draft_or_card_data_still_reads() {
    let (_dir, conn) = db("limited-event-bare");
    run(&conn, 1, "sealed", None, "active", T0);

    let event = event(&conn, "0x:1", NOW, UTC)
        .expect("event")
        .expect("found");
    assert!(event.title.contains("Sealed"), "{}", event.title);
    assert_eq!(event.subtitle, "Unknown set");
    assert_eq!(event.picks_made, 0);
    assert_eq!(event.cover_art, None);
    assert_eq!(event.draft_state, "none");
    assert_eq!(event.packs, Some(6));
    let pages: Vec<LimitedPage> = event.nav.iter().map(|item| item.page).collect();
    assert_eq!(
        pages,
        vec![LimitedPage::Deck, LimitedPage::Matches, LimitedPage::Cards]
    );
}

#[test]
fn unknown_and_constructed_keys_are_not_found() {
    let (_dir, conn) = db("limited-not-found");
    run(&conn, 1, "constructed", None, "complete", T0);
    played(&conn, 1, "q-c1", "CMODERN", "win", T0, None, &["win"]);
    for key in ["0x:1", "q-nope", "", "draft:q-nope"] {
        assert_eq!(event(&conn, key, NOW, UTC).expect("event"), None, "{key}");
        assert_eq!(
            draft_page(&conn, key, None, NOW, UTC).expect("draft"),
            None,
            "{key}"
        );
        assert_eq!(
            deck_page(&conn, key, NOW, UTC).expect("deck"),
            None,
            "{key}"
        );
        assert_eq!(
            cards_page(&conn, key, NOW, UTC).expect("cards"),
            None,
            "{key}"
        );
        assert_eq!(
            matches_page(&conn, key, NOW, UTC).expect("matches"),
            None,
            "{key}"
        );
        assert_eq!(
            cross_drafts(&conn, key, NOW, UTC).expect("cross"),
            None,
            "{key}"
        );
        assert_eq!(
            match_page(&conn, key, "q-c1", &[], NOW, UTC).expect("match"),
            None,
            "{key}"
        );
    }
}

// ---- Draft (0.x DraftControllerTest) ---------------------------------------------

#[test]
fn the_draft_page_has_every_pick_and_its_cards() {
    let (_dir, conn) = db("limited-draft");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    draft(&conn, 1, 1, "finished", Some(T0));
    pick(
        &conn,
        1,
        &PickSeed {
            ordinal: 1,
            available: vec![1, 2],
            picked: Some(1),
            shown_at: Some(T0),
            picked_at: Some(T0 + 26 * SECOND),
            deadline_at: Some(T0 + 69 * SECOND),
            reservations: 2,
        },
    );
    cards(
        &conn,
        "(1, 'Qbard', 'Creature', 'common', 'W', 'W', 'QHB', 'Q Hobbit', 'o-1', NULL)",
    );

    let page = draft_page(&conn, "0x:1", Some(1), NOW, UTC)
        .expect("draft")
        .expect("found");
    assert!(!page.redirect_to_deck);
    assert_eq!(page.selected_ordinal, Some(1));
    let review = page.review.expect("a review");
    assert_eq!(review.picks.len(), 1);
    let first = &review.picks[0];
    assert_eq!(first.label, "P1p1");
    assert_eq!(first.available, vec![1, 2]);
    assert_eq!(first.picked_catalog_id, Some(1));
    assert_eq!(first.elapsed_seconds, Some(26));
    assert_eq!(first.margin_seconds, Some(43));
    assert!(first.indecisive);
    assert_eq!(first.reservations.len(), 2);
    assert_eq!(first.reservations[0].at_seconds, Some(0));
    assert_eq!(review.cards[&1].name, "Qbard");
    assert!(review.cards[&1].resolved);
    // No card data: the id stands in.
    assert_eq!(review.cards[&2].name, "#2");
    assert!(!review.cards[&2].resolved);
    assert_eq!(review.header.picks_made, 1);
    assert_eq!(review.header.seat_count, 8);
    assert_eq!(review.header.state, "finished");
    assert_eq!(
        review.header.colors_picked,
        vec![ColorCountView {
            color: "W".to_string(),
            count: 1
        }]
    );
    assert_eq!(review.signals.len(), 5);
}

#[test]
fn a_draft_run_without_a_draft_has_no_review() {
    let (_dir, conn) = db("limited-draft-none");
    run(&conn, 1, "draft", None, "active", T0);
    let page = draft_page(&conn, "0x:1", None, NOW, UTC)
        .expect("draft")
        .expect("found");
    assert_eq!(page.review, None);
    assert_eq!(page.selected_ordinal, None);
}

#[test]
fn an_unknown_pick_falls_back_to_the_first() {
    let (_dir, conn) = db("limited-draft-fallback");
    run(&conn, 1, "draft", None, "active", T0);
    draft(&conn, 1, 1, "picking", Some(NOW - MINUTE));
    pick(&conn, 1, &pick_seed(4, Some(1)));
    pick(&conn, 1, &pick_seed(5, Some(1)));
    let page = draft_page(&conn, "0x:1", Some(99), NOW, UTC)
        .expect("draft")
        .expect("found");
    assert_eq!(page.selected_ordinal, Some(4));
    let page = draft_page(&conn, "0x:1", Some(5), NOW, UTC)
        .expect("draft")
        .expect("found");
    assert_eq!(page.selected_ordinal, Some(5));
}

#[test]
fn cross_draft_stats_come_from_the_other_drafts_of_the_set() {
    let (_dir, conn) = db("limited-cross");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    draft(&conn, 1, 1, "finished", Some(T0));
    run(&conn, 2, "draft", Some("QHB"), "complete", T0 - DAY);
    draft(&conn, 2, 2, "finished", Some(T0 - DAY));
    pick(&conn, 2, &pick_seed(1, Some(2)));
    cards(
        &conn,
        "(2, 'Qbard', 'Creature', 'common', 'W', 'W', 'QHB', 'Q Hobbit', 'bard', NULL)",
    );

    // Draft 1 saw card 2 (with card data) and card 77 (without).
    pick(&conn, 1, &pick_seed(1, Some(2)));
    pick(&conn, 1, &pick_seed(2, Some(77)));

    // One read gives every card of the draft.
    let all = cross_drafts(&conn, "0x:1", NOW, UTC)
        .expect("cross")
        .expect("found");
    assert_eq!(all.set_code.as_deref(), Some("QHB"));
    let ids: Vec<u32> = all.cards.iter().map(|card| card.catalog_id).collect();
    assert_eq!(ids, vec![2, 77]);
    let view = &all.cards[0];
    assert_eq!(view.set_code.as_deref(), Some("QHB"));
    assert_eq!(view.oracle_id.as_deref(), Some("bard"));
    let stats = view.stats.clone().expect("seen before");
    assert_eq!(stats.times_taken, 1);
    assert_eq!(stats.drafts, 1);
    assert_eq!(stats.avg_ordinal, Some(1.0));

    // A card with no card data has no oracle id to compare by.
    let unknown = &all.cards[1];
    assert_eq!(
        (unknown.oracle_id.clone(), unknown.stats.clone()),
        (None, None)
    );

    // The other run counts only the drafts that are not its own.
    let own = cross_drafts(&conn, "0x:2", NOW, UTC)
        .expect("cross")
        .expect("found");
    let own_stats = own.cards[0].stats.clone().expect("seen in draft 1");
    assert_eq!(own_stats.drafts, 1);
    assert_eq!(own_stats.times_taken, 1);
}

#[test]
fn sealed_has_no_draft_page() {
    let (_dir, conn) = db("limited-draft-sealed");
    run(&conn, 1, "sealed", Some("QMS"), "active", T0);
    let page = draft_page(&conn, "0x:1", None, NOW, UTC)
        .expect("draft")
        .expect("found");
    assert!(page.redirect_to_deck);
    assert_eq!(page.review, None);
}

// ---- Deck (0.x DeckControllerTest) ------------------------------------------------

#[test]
fn the_deck_page_of_a_run_without_registered_decks_is_empty() {
    let (_dir, conn) = db("limited-deck-empty");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    let deck = deck_page(&conn, "0x:1", NOW, UTC)
        .expect("deck")
        .expect("found");
    assert_eq!(deck.summary.version_count, 0);
    assert!(deck.versions.is_empty());
    assert!(deck.games.is_empty());
}

#[test]
fn the_deck_page_builds_the_evolution() {
    let (_dir, conn) = db("limited-deck");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    draft(&conn, 1, 1, "finished", Some(T0 - HOUR));
    pick(&conn, 1, &pick_seed(1, Some(1)));
    cards(
        &conn,
        "(1, 'Qbard', 'Creature', 'common', 'W', 'W', 'QHB', 'Q Hobbit', 'o-1', NULL)",
    );
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        T0 - 30 * MINUTE,
        None,
        &["win", "win"],
    );
    registered(&conn, 1, "q-m1", &[(1, 2, false)], T0 - 30 * MINUTE);

    let deck = deck_page(&conn, "0x:1", NOW, UTC)
        .expect("deck")
        .expect("found");
    assert_eq!(deck.summary.version_count, 1);
    assert_eq!(deck.summary.main_spells, 2);
    assert_eq!(deck.summary.drafted, 1);
    assert!(deck.versions[0].is_current);
    assert_eq!(deck.versions[0].match_labels, vec!["Match 1".to_string()]);
    assert_eq!(deck.cards[&1].name, "Qbard");
    assert_eq!(deck.games[0].result.as_deref(), Some("W"));
    assert_eq!(
        deck.games[0].games[0].note.as_deref(),
        Some("registered deck")
    );
    assert_eq!(
        deck.games[0].games[1].note.as_deref(),
        Some("no board data")
    );
    assert!(!deck.pool.groups.is_empty());
}

// ---- Cards (0.x CardsControllerTest) ------------------------------------------------

#[test]
fn the_cards_page_of_a_run_without_picks_is_empty() {
    let (_dir, conn) = db("limited-cards-empty");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    let table = cards_page(&conn, "0x:1", NOW, UTC)
        .expect("cards")
        .expect("found");
    assert_eq!(table.summary.distinct, 0);
    assert!(table.rows.is_empty());
}

#[test]
fn the_cards_page_has_a_row_per_card_with_cast_stats() {
    let (mut _dir, mut conn) = db("limited-cards");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    draft(&conn, 1, 1, "finished", Some(T0 - HOUR));
    pick(&conn, 1, &pick_seed(1, Some(1001)));
    cards(
        &conn,
        "(1001, 'Qbard', 'Creature', 'common', 'W', 'W', 'QHB', 'Q Hobbit', 'bard', NULL)",
    );
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        T0 - 30 * MINUTE,
        None,
        &["win"],
    );
    registered(&conn, 1, "q-m1", &[(1001, 1, false)], T0 - 30 * MINUTE);
    // 0.x's decoded game log: Qlocal casts Qbard (catalog id 1001) in game 1.
    let detail = ImportedDetail {
        games: Vec::new(),
        log: vec![
            DetailLogEntry {
                at_ms: T0 - 29 * MINUTE,
                message: "@P@PQlocal joined the game.".to_string(),
            },
            DetailLogEntry {
                at_ms: T0 - 29 * MINUTE,
                message: "@P@PQopponent joined the game.".to_string(),
            },
            DetailLogEntry {
                at_ms: T0 - 28 * MINUTE,
                message: "@PQlocal casts @[Qbard@:2002,100:@].".to_string(),
            },
        ],
    };
    store::imported_detail::import_detail(&mut conn, "q-m1", &detail).expect("detail");

    let table = cards_page(&conn, "0x:1", NOW, UTC)
        .expect("cards")
        .expect("found");
    assert_eq!(table.summary.distinct, 1);
    assert_eq!(table.summary.games, 1);
    let row = &table.rows[0];
    assert_eq!(row.catalog_id, 1001);
    assert_eq!(row.labels, vec!["P1p1".to_string()]);
    assert_eq!(row.status, "main");
    assert_eq!((row.games_cast, row.cast_won, row.cast_lost), (1, 1, 0));
    assert_eq!(row.win_pct_cast, Some(100));
    assert_eq!(table.cards[&1001].name, "Qbard");
}

// ---- Matches (0.x MatchesControllerTest, MatchControllerTest) ---------------------

#[test]
fn the_matches_page_has_the_runs_complete_matches_and_kpis() {
    let (_dir, conn) = db("limited-matches");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        T0 - 50 * MINUTE,
        Some(T0 - 40 * MINUTE),
        &["win", "win"],
    );
    played(
        &conn,
        1,
        "q-m2",
        "DQHBQHBQHB",
        "loss",
        T0 - 30 * MINUTE,
        Some(T0 - 18 * MINUTE),
        &["loss", "win", "loss"],
    );
    played(&conn, 0, "q-other", "DQHBQHBQHB", "win", T0, None, &["win"]);

    let page = matches_page(&conn, "0x:1", NOW, UTC)
        .expect("matches")
        .expect("found");
    let tokens: Vec<&str> = page.matches.iter().map(|row| row.token.as_str()).collect();
    assert_eq!(tokens, vec!["q-m1", "q-m2"]);
    assert_eq!((page.kpis.wins, page.kpis.losses), (1, 1));
    assert_eq!((page.kpis.game_wins, page.kpis.game_losses), (3, 2));
    assert_eq!(page.kpis.total_minutes, Some(32));
    assert_eq!(page.kpis.queued_at_ms, Some(T0 - 50 * MINUTE));
    assert_eq!(page.kpis.finished_at_ms, Some(T0 - 18 * MINUTE));
    assert_eq!(page.kpis.on_play.wins + page.kpis.on_play.losses, 2);
}

#[test]
fn a_run_without_complete_matches_has_an_empty_matches_page() {
    let (_dir, conn) = db("limited-matches-empty");
    run(&conn, 1, "sealed", None, "active", T0);
    let page = matches_page(&conn, "0x:1", NOW, UTC)
        .expect("matches")
        .expect("found");
    assert!(page.matches.is_empty());
    assert_eq!((page.kpis.wins, page.kpis.losses), (0, 0));
    assert_eq!(page.kpis.total_minutes, None);
}

#[test]
fn a_limited_opponent_is_labelled_with_the_colours_they_showed() {
    let (_dir, mut conn) = db("limited-matches-colours");
    cards(
        &conn,
        "(8001, 'Qblack', 'Creature', 'common', 'B', 'B', 'QHB', NULL, NULL, NULL),
         (8002, 'Qgreen', 'Creature', 'common', 'G', 'G', 'QHB', NULL, NULL, NULL),
         (8003, 'Qrock', 'Artifact', 'common', '', '', 'QHB', NULL, NULL, NULL)",
    );
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        T0 - 50 * MINUTE,
        None,
        &["win"],
    );
    played(
        &conn,
        1,
        "q-m2",
        "DQHBQHBQHB",
        "loss",
        T0 - 40 * MINUTE,
        None,
        &["loss"],
    );
    played(
        &conn,
        1,
        "q-m3",
        "DQHBQHBQHB",
        "loss",
        T0 - 30 * MINUTE,
        None,
        &["loss"],
    );
    let shown = |ids: &[(u32, u32)]| ImportedDetail {
        games: vec![ImportedGameDetail {
            mtgo_game_id: None,
            game_number: 1,
            local_deck: Vec::new(),
            opponent_cards: ids
                .iter()
                .map(|(id, quantity)| DetailCard {
                    mtgo_id: *id,
                    quantity: *quantity,
                    sideboard: false,
                })
                .collect(),
            frames: Vec::new(),
            ..ImportedGameDetail::default()
        }],
        log: Vec::new(),
    };
    store::imported_detail::import_detail(&mut conn, "q-m1", &shown(&[(8002, 2), (8001, 1)]))
        .expect("detail");
    store::imported_detail::import_detail(&mut conn, "q-m3", &shown(&[(8003, 1)])).expect("detail");

    let page = matches_page(&conn, "0x:1", NOW, UTC)
        .expect("matches")
        .expect("found");
    let colours: Vec<Option<&str>> = page
        .matches
        .iter()
        .map(|row| row.opponent_colors.as_deref())
        .collect();
    assert_eq!(colours, vec![Some("B,G"), None, None]);
    // The index rows read no match events: an expanded row asks for them.
    let view = the_index(&conn, &index_query(None, None, None));
    assert!(
        view.rows[0]
            .matches
            .iter()
            .all(|row| row.opponent_colors.is_none())
    );
    let expanded = run_colours(&conn, "0x:1", NOW, UTC)
        .expect("colours")
        .expect("found");
    assert_eq!(
        expanded,
        vec![
            LimitedMatchColoursView {
                token: "q-m1".to_string(),
                opponent_colors: Some("B,G".to_string()),
            },
            LimitedMatchColoursView {
                token: "q-m2".to_string(),
                opponent_colors: None,
            },
            LimitedMatchColoursView {
                token: "q-m3".to_string(),
                opponent_colors: None,
            },
        ]
    );
    assert_eq!(
        run_colours(&conn, "0x:404", NOW, UTC).expect("colours"),
        None
    );
}

#[test]
fn an_unreadable_match_event_only_loses_that_matchs_colours() {
    let (_dir, mut conn) = db("limited-matches-bad-event");
    cards(
        &conn,
        "(8001, 'Qblack', 'Creature', 'common', 'B', 'B', 'QHB', NULL, NULL, NULL)",
    );
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        T0 - 50 * MINUTE,
        None,
        &["win"],
    );
    played(
        &conn,
        1,
        "q-m2",
        "DQHBQHBQHB",
        "loss",
        T0 - 40 * MINUTE,
        None,
        &["loss"],
    );
    let detail = ImportedDetail {
        games: vec![ImportedGameDetail {
            mtgo_game_id: None,
            game_number: 1,
            local_deck: Vec::new(),
            opponent_cards: vec![DetailCard {
                mtgo_id: 8001,
                quantity: 1,
                sideboard: false,
            }],
            frames: Vec::new(),
            ..ImportedGameDetail::default()
        }],
        log: Vec::new(),
    };
    store::imported_detail::import_detail(&mut conn, "q-m1", &detail).expect("detail");
    // q-m2 is a log match with a stored game state that cannot be decoded.
    conn.execute("UPDATE matches SET source = 'log' WHERE token = 'q-m2'", [])
        .expect("log match");
    conn.execute(
        "INSERT INTO log_files (id, path, identity_hash, head_hash, head_len, day_anchor,
             timezone, clock_date, first_seen_at, last_seen_at)
         VALUES (1, 'q.log', 'q', 'q', 0, '2026-07-01', 'UTC', '2026-07-01', 0, 0)",
        [],
    )
    .expect("log file");
    conn.execute(
        "INSERT INTO log_events (log_file_id, byte_offset, kind, match_token, logged_at, raw,
             compressed)
         VALUES (1, 0, 'game_state_update', 'q-m2', 1, x'00ff00ff', 1)",
        [],
    )
    .expect("bad event");

    let page = matches_page(&conn, "0x:1", NOW, UTC)
        .expect("matches")
        .expect("found");
    let colours: Vec<(&str, Option<&str>)> = page
        .matches
        .iter()
        .map(|row| (row.token.as_str(), row.opponent_colors.as_deref()))
        .collect();
    assert_eq!(colours, vec![("q-m1", Some("B")), ("q-m2", None)]);
    let expanded = run_colours(&conn, "0x:1", NOW, UTC)
        .expect("colours")
        .expect("found");
    assert_eq!(expanded.len(), 2);
    assert_eq!(expanded[0].opponent_colors.as_deref(), Some("B"));
    assert_eq!(expanded[1].opponent_colors, None);
}

#[test]
fn a_match_page_shows_only_a_match_of_the_run() {
    let (_dir, conn) = db("limited-match");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    run(&conn, 2, "draft", Some("QHB"), "active", T0 - 2 * HOUR);
    played(
        &conn,
        1,
        "q-m1",
        "DQHBQHBQHB",
        "win",
        T0 - 50 * MINUTE,
        None,
        &["win"],
    );
    played(
        &conn,
        2,
        "q-m2",
        "DQHBQHBQHB",
        "win",
        T0 - 90 * MINUTE,
        None,
        &["win"],
    );

    let shown = match_page(&conn, "0x:1", "q-m1", &[], NOW, UTC)
        .expect("match")
        .expect("found");
    assert_eq!(shown.token, "q-m1");
    assert!(shown.imported);
    assert_eq!(shown.games.len(), 1);
    assert_eq!(
        match_page(&conn, "0x:1", "q-m2", &[], NOW, UTC).expect("match"),
        None
    );
    assert_eq!(
        match_page(&conn, "0x:1", "q-nope", &[], NOW, UTC).expect("match"),
        None
    );
}

// ---- Pick notes (0.x UpdatePickNoteControllerTest) ---------------------------------

/// Waits for `check` to hold, failing after a few seconds.
fn eventually(mut check: impl FnMut() -> bool) {
    let started = std::time::Instant::now();
    while !check() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn pick_notes_are_saved_trimmed_cleared_and_checked() {
    let dir = TestDir::new("limited-notes");
    let (state, _events) = ready_state(&dir);
    {
        let writer = store::open_writer(&state.db_path).expect("writer");
        writer
            .execute(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
                [],
            )
            .expect("account");
        run(&writer, 1, "draft", Some("QHB"), "active", T0);
        draft(&writer, 1, 1, "finished", Some(T0));
        pick(&writer, 1, &pick_seed(7, Some(1)));
        pick(&writer, 1, &pick_seed(12, Some(2)));
        sync(&writer, NOW, UTC);
    }
    let notes = || {
        let conn = store::open_reader(&state.db_path).expect("reader");
        store::limited::pick_notes(&conn, "q-draft-1").expect("notes")
    };

    set_note(&state, "q-draft-1", 7, "  Qbard over the land  ", NOW).expect("saved");
    assert_eq!(
        notes().get(&7).map(String::as_str),
        Some("Qbard over the land")
    );
    // An earlier pick than the newest one.
    set_note(&state, "q-draft-1", 12, "Q took the removal", NOW).expect("saved");
    set_note(&state, "q-draft-1", 7, "", NOW).expect("cleared");
    assert_eq!(notes().get(&7), None);
    assert_eq!(
        notes().get(&12).map(String::as_str),
        Some("Q took the removal")
    );

    for (token, ordinal) in [
        ("q-draft-1", 43),
        ("q-draft-1", 0),
        ("q-draft-1", 8),
        ("q-nope", 1),
    ] {
        assert_eq!(
            set_note(&state, token, ordinal, "Q", NOW),
            Err(CommandError::Refused(
                "This pick no longer exists.".to_string()
            )),
            "{token} {ordinal}"
        );
    }
    assert_eq!(
        set_note(&state, "q-draft-1", 7, &"a".repeat(2001), NOW),
        Err(CommandError::Refused(
            "The note field must not be greater than 2000 characters.".to_string()
        ))
    );
    // The review shows the note and when it was saved.
    let conn = store::open_reader(&state.db_path).expect("reader");
    eventually(|| notes().contains_key(&12));
    let page = draft_page(&conn, "0x:1", Some(12), NOW, UTC)
        .expect("draft")
        .expect("found");
    let review = page.review.expect("review");
    let twelfth = review
        .picks
        .iter()
        .find(|pick| pick.ordinal == 12)
        .expect("pick 12");
    assert_eq!(twelfth.note.as_deref(), Some("Q took the removal"));
    assert_eq!(twelfth.note_saved_at_ms, Some(NOW));
    crate::commands::stop_tracker(&state);
}

// ---- Draft notes (0.x BuildDraftNotesTest) ----------------------------------------

#[test]
fn no_draft_notes_without_a_live_draft() {
    let (_dir, conn) = db("limited-notes-none");
    draft(&conn, 1, 0, "abandoned", Some(NOW - MINUTE));
    assert_eq!(notes_window(&conn, NOW, UTC).expect("notes"), None);
    // A picking draft idle for more than 30 minutes is abandoned too.
    draft(&conn, 2, 0, "picking", Some(NOW - 31 * MINUTE));
    assert_eq!(notes_window(&conn, NOW, UTC).expect("notes"), None);
}

#[test]
fn a_connecting_draft_has_draft_notes_without_picks() {
    let (_dir, conn) = db("limited-notes-connecting");
    draft(&conn, 1, 0, "connecting", Some(NOW - MINUTE));
    let notes = notes_window(&conn, NOW, UTC).expect("notes").expect("live");
    assert_eq!(notes.draft_token, "q-draft-1");
    assert_eq!(notes.run_key, None);
    assert_eq!(notes.state, "connecting");
    assert_eq!(notes.current_ordinal, None);
    assert!(notes.picks.is_empty());
}

#[test]
fn draft_notes_list_every_pick_in_order_with_names_and_notes() {
    let (_dir, mut conn) = db("limited-notes-picks");
    run(&conn, 1, "draft", Some("QHB"), "active", NOW - MINUTE);
    draft(&conn, 1, 1, "picking", Some(NOW - MINUTE));
    pick(
        &conn,
        1,
        &PickSeed {
            ordinal: 3,
            available: (154000..154012).collect(),
            picked: None,
            shown_at: Some(NOW - 10 * SECOND),
            picked_at: None,
            deadline_at: Some(NOW + 41 * SECOND),
            reservations: 0,
        },
    );
    pick(&conn, 1, &pick_seed(1, Some(154042)));
    pick(&conn, 1, &pick_seed(2, Some(999_999)));
    cards(
        &conn,
        "(154042, 'Qbilbo', 'Legendary Creature', 'rare', 'W', 'W', 'QHB', NULL, 'o-b', NULL)",
    );
    for (ordinal, note) in [(1, "note 1"), (3, "lean red")] {
        store::limited::edit_draft_note(
            &mut conn,
            &store::limited::DraftNoteEdit::Set {
                draft_token: "q-draft-1".to_string(),
                ordinal,
                note: note.to_string(),
                now_ms: NOW,
            },
        )
        .expect("note");
    }

    let notes = notes_window(&conn, NOW, UTC).expect("notes").expect("live");
    assert_eq!(notes.run_key.as_deref(), Some("0x:1"));
    assert_eq!(notes.state, "picking");
    assert_eq!(notes.current_ordinal, Some(3));
    let ordinals: Vec<u32> = notes.picks.iter().map(|pick| pick.ordinal).collect();
    assert_eq!(ordinals, vec![1, 2, 3]);
    let first = &notes.picks[0];
    assert_eq!(first.label, "P1p1");
    assert_eq!(first.picked_catalog_id, Some(154042));
    assert_eq!(first.picked_name.as_deref(), Some("Qbilbo"));
    assert_eq!(first.note.as_deref(), Some("note 1"));
    assert_eq!(notes.picks[1].picked_name.as_deref(), Some("#999999"));
    let third = &notes.picks[2];
    assert_eq!(third.label, "P1p3");
    assert_eq!(third.cards_in_pack, 12);
    assert_eq!(third.deadline_at_ms, Some(NOW + 41 * SECOND));
    assert_eq!(third.picked_catalog_id, None);
    assert_eq!(third.picked_name, None);
    assert_eq!(third.note.as_deref(), Some("lean red"));
}

#[test]
fn a_finished_draft_keeps_its_notes_for_30_seconds() {
    let (_dir, conn) = db("limited-notes-grace");
    draft(&conn, 1, 0, "finished", Some(NOW - HOUR));
    conn.execute("UPDATE drafts SET ended_at = ?1", [NOW - 5 * SECOND])
        .expect("ended");
    pick(&conn, 1, &pick_seed(42, Some(154001)));

    let notes = notes_window(&conn, NOW, UTC).expect("notes").expect("live");
    assert_eq!(notes.state, "finished");
    assert_eq!(notes.current_ordinal, Some(42));
    assert_eq!(notes.picks[0].label, "P3p14");
    assert_eq!(
        notes_window(&conn, NOW + 30 * SECOND, UTC).expect("notes"),
        None
    );
}

#[test]
fn a_live_draft_says_when_its_grace_ends() {
    let (_dir, conn) = db("limited-notes-grace-end");
    draft(&conn, 1, 0, "picking", Some(NOW - MINUTE));
    sync(&conn, NOW, UTC);
    let picking = super::live_notes(&conn, NOW).expect("notes").expect("live");
    assert_eq!(picking.view.state, "picking");
    assert_eq!(picking.grace_ends_at_ms, None, "no timer while picking");

    conn.execute(
        "UPDATE drafts SET state = 'finished', abandons_at = NULL, ended_at = ?1",
        [NOW - 5 * SECOND],
    )
    .expect("finished");
    let finished = super::live_notes(&conn, NOW).expect("notes").expect("live");
    assert_eq!(finished.view.state, "finished");
    // Live through the 30th second, gone the millisecond after.
    assert_eq!(finished.grace_ends_at_ms, Some(NOW + 25 * SECOND + 1));
    assert!(
        super::live_notes(&conn, NOW + 25 * SECOND)
            .expect("notes")
            .is_some()
    );
    assert_eq!(
        super::live_notes(&conn, NOW + 25 * SECOND + 1).expect("notes"),
        None
    );
}

// ---- Real logs (0.x LimitedPagesFixtureTest) ---------------------------------------

/// The fixture log read by the tracker into a fresh database.
fn tracked(name: &str, label: &str) -> (TestDir, Connection, i64, Tz) {
    let fixture = load_fixture(name);
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes);
    // Both fixtures were recorded on a UTC+1 machine: line prefixes read an
    // hour past their payloads' UTC times. They are read in the timezone the
    // logs were written in (as 0.x's test pins it), so pick times are real.
    let tz = chrono_tz::Europe::London;
    let config = TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: tz,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: Some(fixture.meta.day_anchor()),
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    let mut now = fixture.meta.now_ms();
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        now,
    )
    .expect("open engine");
    for _ in 0..20_000 {
        let tick = engine.tick(now).expect("tick");
        now += 1;
        if !tick.more {
            break;
        }
    }
    for _ in 0..3 {
        now += 1_000;
        engine.tick(now).expect("tick");
    }
    engine.flush().expect("flush");
    drop(engine);
    let conn = store::open_writer(&config.db_path).expect("writer");
    (dir, conn, fixture.meta.now_ms(), tz)
}

#[test]
fn every_page_reads_the_hobbit_draft() {
    let (_dir, conn, now, tz) = tracked("draft_hobbit", "limited-pages-hobbit");
    let view = index(&conn, &index_query(None, None, None), now, tz).expect("index");
    assert_eq!(view.rows.len(), 1);
    let row = &view.rows[0];
    let key = row.run_key.clone().expect("a run");
    assert_eq!(row.set_code.as_deref(), Some("HOB"));
    assert_eq!(row.kind, "draft");
    assert_eq!(row.title, "HOB Draft");
    assert_eq!((row.picks_made, row.picks_expected), (42, 42));
    assert!(row.deck_registered);
    assert_eq!(row.version_count, 2);
    assert!(row.linked);
    assert_eq!((row.wins, row.losses), (1, 2));
    assert_eq!(
        row.results,
        vec![Some(MarkView::L), Some(MarkView::W), Some(MarkView::L)]
    );
    assert_eq!(row.opponents.len(), 3);
    assert_eq!(
        (row.state.as_str(), row.state_variant.as_str()),
        ("Complete", "success")
    );
    assert_eq!(
        (view.kpis.events, view.kpis.drafts, view.kpis.unlinked),
        (1, 1, 0)
    );
    assert_eq!((view.kpis.match_wins, view.kpis.match_losses), (1, 2));
    assert_eq!(view.kpis.match_win_pct, Some(33));
    assert_eq!(view.kpis.most_drafted_set.as_deref(), Some("HOB"));
    assert_eq!(set_codes_of(&view.sets), vec!["HOB"]);

    let event = event(&conn, &key, now, tz).expect("event").expect("found");
    assert_eq!(event.set_code.as_deref(), Some("HOB"));
    assert_eq!(event.seat_index, Some(7));
    assert_eq!(event.draft_state, "finished");
    assert_eq!(event.subtitle, "Unknown set · League 11039");

    let page = draft_page(&conn, &key, None, now, tz)
        .expect("draft")
        .expect("found");
    assert_eq!(page.selected_ordinal, Some(1));
    let review = page.review.expect("review");
    assert_eq!(review.picks.len(), 42);
    assert_eq!(review.header.seat_index, Some(7));
    assert_eq!(review.header.seat_count, 8);
    assert_eq!(review.header.pack_size, 14);
    assert_eq!(
        (review.header.picks_made, review.header.picks_expected),
        (42, 42)
    );
    assert_eq!(review.header.state, "finished");
    let first = &review.picks[0];
    assert_eq!(first.label, "P1p1");
    assert_eq!((first.pack_number, first.pick_number), (1, 1));
    assert_eq!(first.picked_catalog_id, Some(154_228));
    assert_eq!(first.available.len(), 14);
    assert_eq!(first.wheel_return_ordinal, Some(9));
    assert_eq!(first.wheeled_ids.len(), 6);
    assert_eq!(first.taken_ids.len(), 7);
    assert_eq!(first.elapsed_seconds, Some(26));
    assert_eq!(first.margin_seconds, Some(43));
    assert!(first.indecisive);
    assert_eq!(review.picks[41].label, "P3p14");
    assert_eq!(review.signals.len(), 5);
    assert!(!review.seen_wheel.is_empty());
    // No card data: each card is `#id`, or the name the game log gave its
    // stub (Plan S task 1), with nothing else known (S1-8: the exact name).
    let stub_names: std::collections::BTreeMap<u32, String> = {
        let mut stmt = conn
            .prepare("SELECT catalog_id, name FROM cards WHERE name IS NOT NULL")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("names")
            .map(|row| row.expect("row"))
            .collect()
    };
    for (catalog_id, card) in &review.cards {
        let expected = stub_names
            .get(catalog_id)
            .cloned()
            .unwrap_or_else(|| format!("#{catalog_id}"));
        assert_eq!(card.name, expected, "{catalog_id}");
        assert!(card.card_type.is_none() && card.image.is_none() && card.oracle_id.is_none());
    }

    let deck = deck_page(&conn, &key, now, tz)
        .expect("deck")
        .expect("found");
    assert_eq!(deck.summary.version_count, 2);
    assert_eq!(deck.summary.drafted, 42);
    assert_eq!(deck.versions[0].match_labels, vec!["Match 1".to_string()]);
    assert!(!deck.versions[0].is_current);
    assert_eq!(
        deck.versions[1].match_labels,
        vec!["Match 2".to_string(), "Match 3".to_string()]
    );
    assert!(deck.versions[1].is_current);
    let added: Vec<(u32, u32)> = deck.versions[1]
        .diff_main
        .added
        .iter()
        .map(|card| (card.catalog_id, card.quantity))
        .collect();
    assert_eq!(added, vec![(154_028, 1), (154_228, 2)]);
    let removed: Vec<(u32, u32)> = deck.versions[1]
        .diff_main
        .removed
        .iter()
        .map(|card| (card.catalog_id, card.quantity))
        .collect();
    assert_eq!(removed, vec![(153_894, 1), (153_900, 1), (154_000, 1)]);
    assert_eq!(deck.games.len(), 3);
    assert_eq!(deck.games[0].label, "Match 1");
    assert_eq!(deck.games[0].result.as_deref(), Some("L"));
    assert_eq!(
        deck.games[0].games[0].note.as_deref(),
        Some("registered deck")
    );
    assert_eq!(deck.games[0].games[1].note.as_deref(), Some("no changes"));
    assert!(!deck.pool.groups.is_empty());

    let matches = matches_page(&conn, &key, now, tz)
        .expect("matches")
        .expect("found");
    assert_eq!(matches.matches.len(), 3);
    assert_eq!((matches.kpis.wins, matches.kpis.losses), (1, 2));
    assert_eq!((matches.kpis.game_wins, matches.kpis.game_losses), (1, 3));

    let table = cards_page(&conn, &key, now, tz)
        .expect("cards")
        .expect("found");
    assert_eq!(table.summary.distinct, 30);
    assert_eq!(table.summary.games, 4);
    assert_eq!(table.summary.other_drafts, 0);
    assert_eq!(table.rows.len(), 30);
    assert_eq!(table.rows[0].catalog_id, 154_228);
    assert_eq!(
        table.rows[0].labels,
        vec!["P1p1".to_string(), "P2p7".to_string()]
    );
    assert_eq!(table.rows[0].ordinals, vec![1, 21]);
    assert_eq!(table.rows[0].status, "main");
    assert!(!table.cards.is_empty());

    let token = &matches.matches[0].token;
    let shown = match_page(&conn, &key, token, &[], now, tz)
        .expect("match")
        .expect("found");
    assert_eq!(&shown.token, token);

    // The draft finished long before the fixture's now: no notes window.
    assert_eq!(notes_window(&conn, now, tz).expect("notes"), None);
}

#[test]
fn every_page_reads_the_incomplete_draft() {
    let (_dir, conn, now, tz) = tracked("draft_incomplete", "limited-pages-incomplete");
    let view = index(&conn, &index_query(None, None, None), now, tz).expect("index");
    assert_eq!(view.rows.len(), 1);
    let row = &view.rows[0];
    let key = row.run_key.clone().expect("a run");
    assert_eq!(row.set_code.as_deref(), Some("MSH"));
    assert_eq!(row.picks_made, 42);
    assert!(!row.deck_registered);
    assert_eq!(row.version_count, 0);
    assert_eq!(row.results, vec![None, None, None]);
    assert!(row.opponents.is_empty());
    assert_eq!(
        (row.state.as_str(), row.state_variant.as_str()),
        ("Active", "default")
    );
    assert_eq!(row.note, None);
    assert_eq!(view.kpis.events, 1);
    assert_eq!(view.kpis.match_win_pct, None);

    let page = draft_page(&conn, &key, None, now, tz)
        .expect("draft")
        .expect("found");
    let review = page.review.expect("review");
    assert_eq!(review.picks.len(), 42);
    assert_eq!(review.header.seat_index, Some(3));
    assert_eq!(review.header.seat_count, 8);
    assert_eq!(review.header.state, "finished");
    assert_eq!(review.picks[0].label, "P1p1");
    assert_eq!(review.signals.len(), 5);

    let deck = deck_page(&conn, &key, now, tz)
        .expect("deck")
        .expect("found");
    assert_eq!(deck.summary.version_count, 0);
    assert_eq!(deck.summary.drafted, 42);
    assert!(deck.versions.is_empty());
    assert!(deck.games.is_empty());
    assert_eq!(deck.pool.groups[0].cards[0].status, "pool");

    let matches = matches_page(&conn, &key, now, tz)
        .expect("matches")
        .expect("found");
    assert!(matches.matches.is_empty());

    let table = cards_page(&conn, &key, now, tz)
        .expect("cards")
        .expect("found");
    assert_eq!(table.summary.games, 0);
    assert_eq!(table.summary.distinct, 39);
    assert_eq!(table.rows[0].labels, vec!["P1p1".to_string()]);
    assert_eq!(table.rows[0].status, "pool");
}

// ---- Home dashboard fixtures (Most picked, the limited league cards) ---------------

/// A complete draft run of `set` with a draft picking `picked`.
fn picked_run(conn: &Connection, id: i64, set: &str, started_at: i64, picked: &[u32]) {
    run(conn, id, "draft", Some(set), "complete", started_at);
    draft(conn, id, id, "finished", Some(started_at));
    for (index, catalog_id) in picked.iter().enumerate() {
        pick(conn, id, &pick_seed(index as u32 + 1, Some(*catalog_id)));
    }
}

fn pick_counts(view: &LimitedPicksView) -> Vec<(u32, Option<&str>, u32)> {
    view.picks
        .iter()
        .map(|pick| (pick.catalog_id, pick.name.as_deref(), pick.count))
        .collect()
}

// ---- The home dashboard's runs and Most picked (dashboard redesign spec 4.2 to 4.8) ----

#[test]
fn dashboard_runs_carry_colours_cover_and_first_pick() {
    let (_dir, conn) = db("limited-dashboard-runs");
    cards(
        &conn,
        "(101, 'Qbolt', 'Instant', 'rare', 'R', 'R', 'QNW', 'Q New Set', 'o-101', 'https://q/101.jpg'),
         (102, 'Qcounter', 'Instant', 'common', 'U', 'U', 'QNW', 'Q New Set', 'o-102', NULL),
         (103, 'Qmountain', 'Basic Land', 'common', NULL, NULL, 'QNW', 'Q New Set', 'o-103', NULL),
         (104, 'Qgrowth', 'Sorcery', 'common', 'G', 'G', 'QNW', 'Q New Set', 'o-104', NULL)",
    );
    picked_run(&conn, 1, "QNW", NOW - DAY, &[101, 102]);
    played(
        &conn,
        1,
        "q-m1",
        "DQNWQNWQNW",
        "win",
        NOW - DAY,
        None,
        &["win", "win"],
    );
    registered(
        &conn,
        1,
        "q-m1",
        &[
            (101, 2, false),
            (102, 1, false),
            (103, 17, false),
            (104, 1, true),
        ],
        NOW - DAY,
    );
    run(&conn, 2, "sealed", Some("QOD"), "active", NOW - HOUR);

    let data = gather(&conn, NOW, UTC).expect("gather");
    let runs = super::dashboard_runs(&conn, &data, UTC).expect("runs");
    let sealed = &runs[0];
    assert_eq!(sealed.key, "0x:2");
    assert_eq!((sealed.kind, sealed.rounds), (LeagueKind::Sealed, 5));
    assert_eq!(sealed.first_pick.as_deref(), Some("Sealed"));
    assert_eq!(sealed.colors, None, "no registered deck");
    let draft = &runs[1];
    assert_eq!(
        (draft.kind, draft.rounds, draft.wins, draft.losses),
        (LeagueKind::Draft, 3, 1, 0)
    );
    assert_eq!(
        draft.colors,
        Some(vec!["U".to_string(), "R".to_string()]),
        "lands and the sideboard left out"
    );
    assert_eq!(draft.colour_name.as_deref(), Some("Izzet"));
    assert_eq!(draft.subject, "Q New Set · Izzet");
    assert_eq!(draft.set_name.as_deref(), Some("Q New Set"));
    assert_eq!(draft.first_pick.as_deref(), Some("Qbolt"));
    assert_eq!(draft.cover_art.as_deref(), Some("https://q/101.jpg"));
    assert_eq!(draft.match_tokens, vec!["q-m1".to_string()]);
    assert_eq!(draft.last_played_at, NOW - DAY);
}

#[test]
fn most_picked_counts_a_set_or_every_set() {
    let (_dir, conn) = db("limited-most-picked");
    cards(
        &conn,
        "(101, 'Qbolt', 'Instant', 'common', 'R', 'R', 'QNW', 'Q New Set', 'o-101', NULL),
         (102, 'Qcounter', 'Instant', 'common', 'U', 'U', 'QNW', NULL, 'o-102', NULL),
         (201, 'Qold bolt', 'Instant', 'common', 'R', 'R', 'QOD', 'Q Old Set', 'o-201', NULL)",
    );
    picked_run(&conn, 1, "QOD", NOW - 10 * DAY, &[201, 201, 201]);
    picked_run(&conn, 2, "QNW", NOW - DAY, &[101, 102, 101]);
    picked_run(&conn, 3, "QNW", NOW - 2 * HOUR, &[101]);
    let data = gather(&conn, NOW, UTC).expect("gather");

    let newest = super::most_picked(&conn, &data, Some("QNW")).expect("picks");
    assert_eq!(newest.set_code.as_deref(), Some("QNW"));
    assert_eq!(newest.set_name.as_deref(), Some("Q New Set"));
    assert_eq!(
        pick_counts(&newest),
        vec![(101, Some("Qbolt"), 3), (102, Some("Qcounter"), 1)]
    );
    let every = super::most_picked(&conn, &data, None).expect("picks");
    assert_eq!((&every.set_code, &every.set_name), (&None, &None));
    assert_eq!(
        pick_counts(&every),
        vec![
            (101, Some("Qbolt"), 3),
            (201, Some("Qold bolt"), 3),
            (102, Some("Qcounter"), 1)
        ]
    );
    assert_eq!(
        super::picked_sets(&conn, &data).expect("sets"),
        vec![
            PickSetOptionView {
                code: "QNW".to_string(),
                name: "Q New Set".to_string(),
            },
            PickSetOptionView {
                code: "QOD".to_string(),
                name: "Q Old Set".to_string(),
            },
        ]
    );
}

#[test]
fn most_picked_stops_at_six_whatever_the_timeframe() {
    let (_dir, conn) = db("limited-most-picked-six");
    let ids: Vec<u32> = (300..312).collect();
    picked_run(&conn, 1, "QNW", NOW - 3 * 365 * DAY, &ids);
    let data = gather(&conn, NOW, UTC).expect("gather");
    let view = super::most_picked(&conn, &data, Some("QNW")).expect("picks");
    assert_eq!(view.picks.len(), MOST_PICKED);
    // Ties go by catalog id; no card data, no name.
    assert_eq!(view.picks[0].catalog_id, 300);
    assert_eq!(view.picks[0].name, None);
}

/// The home view in a mode, after a sync.
fn home(conn: &Connection, timeframe: &str, format: &str) -> crate::dashboard::HomeDashboardView {
    sync(conn, NOW, UTC);
    crate::dashboard::home_dashboard_view(conn, timeframe, format, NOW, UTC).expect("home")
}

#[test]
fn the_limited_league_card_is_the_run_in_progress_else_the_newest() {
    let (_dir, conn) = db("limited-home-league");
    run(&conn, 1, "constructed", None, "complete", NOW);
    run(&conn, 2, "draft", Some("QLV"), "active", NOW);
    run(&conn, 3, "sealed", Some("QOD"), "complete", NOW - 5 * DAY);
    run(&conn, 4, "draft", Some("QNW"), "dropped", NOW - DAY);
    played(
        &conn,
        4,
        "q-m1",
        "DQNWQNWQNW",
        "win",
        NOW - DAY,
        None,
        &["win", "win"],
    );
    played(
        &conn,
        4,
        "q-m2",
        "DQNWQNWQNW",
        "loss",
        NOW - DAY + HOUR,
        None,
        &["loss", "loss"],
    );
    let live = home(&conn, "alltime", "limited");
    let league = live.league.expect("a limited league");
    assert_eq!(league.key, "0x:2");
    assert_eq!(league.state, RunStateView::Active);
    assert_eq!(league.name, "QLV Draft");
    assert_eq!(league.source, RunSource::Imported);

    let (_dir, conn) = db("limited-home-league-finished");
    run(&conn, 3, "sealed", Some("QOD"), "complete", NOW - 5 * DAY);
    run(&conn, 4, "draft", Some("QNW"), "dropped", NOW - DAY);
    played(
        &conn,
        4,
        "q-m1",
        "DQNWQNWQNW",
        "win",
        NOW - DAY,
        None,
        &["win", "win"],
    );
    let finished = home(&conn, "alltime", "limited")
        .league
        .expect("a limited league");
    assert_eq!(finished.key, "0x:4");
    assert_eq!(finished.state, RunStateView::Dropped);
    assert_eq!(finished.format, "Draft");
    // A finished run shows only the matches it played, as the Leagues page.
    assert_eq!(finished.results, vec![Some(MarkView::W)]);
    assert_eq!(finished.colors, None, "no registered deck");

    let (_dir, conn) = db("limited-home-league-none");
    run(&conn, 1, "constructed", None, "complete", NOW);
    assert_eq!(home(&conn, "alltime", "limited").league, None);
}

#[test]
fn the_home_dashboard_serves_most_picked_and_its_sets() {
    let (_dir, conn) = db("limited-home-most-picked");
    picked_run(&conn, 1, "QNW", NOW - DAY, &[101]);
    cards(
        &conn,
        "(101, 'Qbolt', 'Instant', 'common', 'R', 'R', 'QNW', 'Q New Set', 'o-101', NULL)",
    );
    let limited = home(&conn, "week", "limited");
    let picked = limited.most_picked.expect("most picked");
    assert_eq!(
        picked.sets,
        vec![PickSetOptionView {
            code: "QNW".to_string(),
            name: "Q New Set".to_string(),
        }]
    );
    assert_eq!(picked.picks.set_code.as_deref(), Some("QNW"));
    assert_eq!(picked.picks.picks[0].count, 1);
    assert_eq!(home(&conn, "week", "all").most_picked, None, "Limited only");
}

/// Every Limited page and the home dashboard's limited cards read stored rows: with the log's
/// tables gone, they read the same.
#[test]
fn the_limited_index_reads_no_log_events_or_archives() {
    let (_dir, conn, now, tz) = tracked("draft_hobbit", "limited-no-log-reads");
    sync(&conn, now, tz);
    let query = index_query(None, None, None);
    let view = super::index(&conn, &query, now, tz).expect("index");
    let key = view.rows[0].run_key.clone().expect("a run");
    let read = |conn: &Connection| {
        (
            super::index(conn, &query, now, tz).expect("index"),
            super::event(conn, &key, tz).expect("event"),
            super::draft_page(conn, &key, None).expect("draft"),
            super::deck_page(conn, &key).expect("deck"),
            super::cards_page(conn, &key).expect("cards"),
            super::matches_page(conn, &key, tz).expect("matches"),
            super::run_colours(conn, &key).expect("colours"),
            crate::dashboard::home_dashboard_view(conn, "alltime", "limited", now, tz)
                .expect("home"),
            super::most_picked(conn, &super::gather(conn).expect("gather"), None)
                .expect("most picked"),
            super::notes_window(conn, now).expect("notes"),
        )
    };
    let before = read(&conn);
    assert!(
        before
            .3
            .as_ref()
            .is_some_and(|deck| deck.summary.version_count == 2)
    );
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF;
         DROP TABLE log_events;
         DROP TABLE match_event_archives;",
    )
    .expect("the log's tables gone");
    assert_eq!(read(&conn), before);
}

fn limited_run_for(key: &str, draft_token: Option<&str>, former: &[&str]) -> LimitedRun {
    LimitedRun {
        run: leagues::Run {
            key: key.to_string(),
            token: "league".to_string(),
            event_id: None,
            joined_at: None,
            name: String::new(),
            format: None,
            deck_version_id: None,
            started_at: T0,
            state: RunState::Active,
            dropped_at: None,
            completed_at: None,
            match_tokens: Vec::new(),
            kind: LeagueKind::Draft,
            set_code: None,
            course_id: None,
            draft_token: draft_token.map(str::to_string),
            former_keys: former.iter().map(|key| key.to_string()).collect(),
            rounds: None,
        },
        draft: None,
        matches: Vec::new(),
        registered: Vec::new(),
        source: "log".to_string(),
        notes: None,
        user_state: None,
        stated_pool: None,
        course_record: None,
    }
}

#[test]
fn an_old_draft_key_finds_the_run_that_adopted_the_draft() {
    let gathered = Gathered {
        runs: vec![
            limited_run_for("league:m1", Some("d1"), &[]),
            limited_run_for("league:m2", None, &["draft:d2"]),
        ],
        unlinked: Vec::new(),
    };

    assert_eq!(
        gathered.run("league:m1").map(|run| run.run.key.as_str()),
        Some("league:m1")
    );
    assert_eq!(
        gathered.run("draft:d1").map(|run| run.run.key.as_str()),
        Some("league:m1")
    );
    assert_eq!(
        gathered.run("draft:d2").map(|run| run.run.key.as_str()),
        Some("league:m2")
    );
    assert!(gathered.run("draft:other").is_none());
}

// ---- The player's status ------------------------------------------------------

#[test]
fn limited_rows_and_cards_show_the_players_status() {
    let (_dir, conn) = db("limited-user-state");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    run(&conn, 2, "draft", Some("QHB"), "active", T0);
    conn.execute(
        "UPDATE leagues SET user_state = 'partial', notes = 'Left early' WHERE run_key = '0x:1'",
        [],
    )
    .expect("status");

    let view = the_index(&conn, &index_query(None, None, None));
    let row = view
        .rows
        .iter()
        .find(|row| row.run_key.as_deref() == Some("0x:1"))
        .expect("row");
    assert_eq!(row.state, "Dropped");
    assert_eq!(row.user_state, Some(state_view(RunState::Partial)));
    assert_eq!(row.auto_state, state_view(RunState::Active));
    assert_eq!(row.notes.as_deref(), Some("Left early"));
    let automatic = view
        .rows
        .iter()
        .find(|row| row.run_key.as_deref() == Some("0x:2"))
        .expect("row");
    assert_eq!(automatic.state, "Active");
    assert_eq!(automatic.user_state, None);
    assert_eq!(automatic.auto_state, state_view(RunState::Active));
    assert_eq!(automatic.notes, None);

    sync(&conn, NOW, UTC);
    let data = super::gather(&conn).expect("gather");
    let card = run_card(&conn, &data, "0x:1", UTC)
        .expect("card")
        .expect("run");
    assert_eq!(card.state, state_view(RunState::Partial));
    assert_eq!(card.user_state, Some(state_view(RunState::Partial)));
    assert_eq!(card.auto_state, state_view(RunState::Active));
    let stored = data.run("0x:1").expect("run");
    assert_eq!(stored.run.state, RunState::Active);
    assert_eq!(stored.shown_state(), RunState::Partial);
}

#[test]
fn an_unlinked_draft_row_has_no_player_status() {
    let (_dir, conn) = db("limited-user-state-unlinked");
    run(&conn, 1, "draft", Some("QHB"), "active", T0);
    draft(&conn, 5, 0, "finished", Some(T0));
    let view = the_index(&conn, &index_query(None, None, None));
    let row = view
        .rows
        .iter()
        .find(|row| row.run_key.is_none())
        .expect("unlinked");
    assert_eq!(row.user_state, None);
    assert_eq!(row.notes, None);
}

#[test]
fn the_kpis_follow_the_players_status() {
    let (_dir, conn) = db("limited-user-state-kpis");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    run(&conn, 2, "draft", Some("QHB"), "complete", T0);
    for (id, token) in [(1, "q-k1"), (2, "q-k2")] {
        played(
            &conn,
            id,
            token,
            "DQHBQHBQHB",
            "win",
            T0,
            Some(T0 + HOUR),
            &["win", "win"],
        );
    }
    let set_status = |key: &str, status: &str| {
        conn.execute(
            "UPDATE leagues SET user_state = ?1 WHERE run_key = ?2",
            [status, key],
        )
        .expect("status");
    };
    let completed = || {
        the_index(&conn, &index_query(None, None, None))
            .kpis
            .completed_runs
    };
    // The fold completed one run; the player completes the other too.
    assert_eq!(completed(), 1);
    set_status("0x:1", "complete");
    assert_eq!(completed(), 2);
    // A run the fold completed, set to Dropped by the player, no longer counts.
    set_status("0x:2", "partial");
    assert_eq!(completed(), 1);
}

#[test]
fn the_empty_run_note_follows_the_players_status() {
    let (_dir, conn) = db("limited-user-state-note");
    run(&conn, 1, "draft", Some("QHB"), "active", T0 - HOUR);
    conn.execute(
        "UPDATE leagues SET user_state = 'complete' WHERE run_key = '0x:1'",
        [],
    )
    .expect("complete");
    let view = the_index(&conn, &index_query(None, None, None));
    let row = view
        .rows
        .iter()
        .find(|row| row.run_key.as_deref() == Some("0x:1"))
        .expect("row");
    assert_eq!(
        row.note.as_deref(),
        Some("Draft finished, league ended without play")
    );
}

fn set_codes_of(sets: &[PickSetOptionView]) -> Vec<&str> {
    sets.iter().map(|set| set.code.as_str()).collect()
}

// ---- Arena runs ------------------------------------------------------------------

#[test]
fn the_limited_pages_show_the_fixtures_arena_runs() {
    let (_dir, conn, _) =
        crate::matches::tests::tracked_arena("sealed-direct-fast-login", "limited-arena-runs");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'arena')
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [store::platform::PLATFORM_CONTEXT],
    )
    .expect("switch to Arena");

    let view = index(&conn, &index_query(None, None, None), NOW, UTC).expect("index");
    let keys: Vec<&str> = view
        .rows
        .iter()
        .filter_map(|row| row.run_key.as_deref())
        .collect();
    assert_eq!(keys.len(), 2, "both Arena runs: {keys:?}");
    assert!(keys.iter().all(|key| key.starts_with("arena:")));
    let direct = view
        .rows
        .iter()
        .find(|row| row.kind == "sealed")
        .expect("the Direct");
    assert_eq!((direct.wins, direct.losses), (5, 0));
    let key = direct.run_key.clone().expect("a run");

    let event = event(&conn, &key, NOW, UTC)
        .expect("event")
        .expect("the Direct");
    assert_eq!((event.wins, event.losses), (5, 0));
    assert!(!event.editable);
    assert!(!event.picks_recorded);

    let deck = deck_page(&conn, &key, NOW, UTC)
        .expect("deck")
        .expect("the Direct");
    let pool: Vec<&PoolCardView> = deck
        .pool
        .groups
        .iter()
        .flat_map(|group| &group.cards)
        .collect();
    let total: u32 = pool.iter().map(|card| card.quantity).sum();
    assert_eq!(total, 78, "the stated pool");
    // A card the registered deck holds outside the stated pool is a basic
    // added in deckbuilding: typed as one, it would show if the pool were
    // read off the deck.
    let in_pool: BTreeSet<u32> = pool.iter().map(|card| card.catalog_id).collect();
    let basic = deck
        .versions
        .iter()
        .flat_map(|version| version.main_cards.iter().chain(&version.side_cards))
        .map(|card| card.catalog_id)
        .find(|catalog_id| !in_pool.contains(catalog_id))
        .expect("an added basic");
    conn.execute(
        "INSERT INTO cards (catalog_id, name, type) VALUES (?1, 'Forest', 'Basic Land')
         ON CONFLICT (catalog_id) DO UPDATE SET name = excluded.name, type = excluded.type",
        [basic],
    )
    .expect("basic land row");
    let deck = deck_page(&conn, &key, NOW, UTC)
        .expect("deck")
        .expect("the Direct");
    let mut total = 0;
    for card in deck.pool.groups.iter().flat_map(|group| &group.cards) {
        total += card.quantity;
        let card_type = deck
            .cards
            .get(&card.catalog_id)
            .and_then(|view| view.card_type.clone())
            .unwrap_or_default();
        assert!(
            !card_type.contains("Basic"),
            "{} is a basic land",
            card.catalog_id
        );
    }
    assert_eq!(total, 78);
}

/// An Arena match's game structure is `bo1` or `bo3`, not a format: an
/// Arena run is titled by its stored name and labelled by its set code.
#[test]
fn an_arena_run_is_titled_by_its_name_and_labelled_by_its_set() {
    let (_dir, conn, _) =
        crate::matches::tests::tracked_arena("sealed-direct-fast-login", "limited-arena-titles");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'arena')
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [store::platform::PLATFORM_CONTEXT],
    )
    .expect("switch to Arena");
    let (key, name, set_code): (String, String, String) = conn
        .query_row(
            "SELECT run_key, name, set_code FROM leagues WHERE kind = 'sealed'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("the Direct");
    assert!(!name.is_empty());

    let view = index(&conn, &index_query(None, None, None), NOW, UTC).expect("index");
    let row = view
        .rows
        .iter()
        .find(|row| row.run_key.as_deref() == Some(key.as_str()))
        .expect("the Direct row");
    assert_eq!(row.title, name);
    let most_drafted = view.kpis.most_drafted_set.as_deref().unwrap_or_default();
    assert!(!most_drafted.starts_with("bo"), "{most_drafted}");

    let event = event(&conn, &key, NOW, UTC)
        .expect("event")
        .expect("the Direct");
    assert!(event.title.starts_with(&name), "{}", event.title);
    assert_eq!(event.set_label.as_deref(), Some(set_code.as_str()));
}

#[test]
fn the_arena_draft_and_sealed_runs_open_their_own_pages() {
    let (_dir, conn, _) =
        crate::matches::tests::tracked_arena("sealed-direct-fast-login", "limited-arena-draft");
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, 'arena')
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [store::platform::PLATFORM_CONTEXT],
    )
    .expect("switch to Arena");

    let draft_key = "arena:dfed50e8-11c3-431e-a6c2-d7b4efdfed8e";
    let draft = draft_page(&conn, draft_key, None, NOW, UTC)
        .expect("draft page")
        .expect("the Arena draft run");
    assert!(draft.review.is_none());
    assert!(!draft.redirect_to_deck);
    let event = event(&conn, draft_key, NOW, UTC)
        .expect("event")
        .expect("the Arena draft run");
    assert!(!event.picks_recorded);
    let deck = deck_page(&conn, draft_key, NOW, UTC)
        .expect("deck")
        .expect("the Arena draft run");
    let total: u32 = deck
        .pool
        .groups
        .iter()
        .flat_map(|group| &group.cards)
        .map(|card| card.quantity)
        .sum();
    assert_eq!(total, 42, "the draft pool");

    let sealed_key: String = conn
        .query_row(
            "SELECT run_key FROM leagues WHERE kind = 'sealed'",
            [],
            |row| row.get(0),
        )
        .expect("the Direct");
    let sealed = draft_page(&conn, &sealed_key, None, NOW, UTC)
        .expect("draft page")
        .expect("the Direct");
    assert!(sealed.redirect_to_deck);
}

// #834: an Arena run takes a status (written with its edit time, so it
// syncs, and shown over the course's state); every other edit is refused.
#[test]
fn an_arena_run_takes_a_status_and_refuses_other_edits() {
    let (_dir, mut conn, _) =
        crate::matches::tests::tracked_arena("sealed-direct-fast-login", "limited-arena-edit");
    let key: String = conn
        .query_row(
            "SELECT run_key FROM leagues WHERE kind = 'sealed'",
            [],
            |row| row.get(0),
        )
        .expect("the Direct");
    // `user_state`, `user_dropped_at`, `edited_at`, `notes`.
    type Player = (Option<String>, Option<i64>, Option<i64>, Option<String>);
    let player = |conn: &store::Connection| -> Player {
        conn.query_row(
            "SELECT user_state, user_dropped_at, edited_at, notes FROM leagues WHERE run_key = ?1",
            [&key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("the row")
    };
    let before = player(&conn);
    for edit in [
        store::leagues::LeagueEdit::SetNotes {
            run_key: key.clone(),
            notes: "a note".to_string(),
            now_ms: NOW,
        },
        store::leagues::LeagueEdit::Delete {
            run_key: key.clone(),
            matches: store::leagues::DeleteMatches::KeepAsNonLeague,
            now_ms: NOW,
        },
    ] {
        let outcome = store::leagues::edit_league(&mut conn, &edit, UTC).expect("edit");
        assert_eq!(outcome.refusal(), Some("Arena runs cannot be edited yet."));
    }
    assert_eq!(player(&conn), before);

    let status = |status| store::leagues::LeagueEdit::SetStatus {
        run_key: key.clone(),
        status,
        now_ms: NOW,
    };
    let outcome = store::leagues::edit_league(
        &mut conn,
        &status(Some(store::leagues::UserState::Partial)),
        UTC,
    )
    .expect("edit");
    assert_eq!(outcome, store::leagues::LeagueEditOutcome::Done);
    assert_eq!(
        player(&conn),
        (Some("partial".to_string()), None, Some(NOW), None)
    );
    let view = index(&conn, &index_query(None, None, None), NOW, UTC).expect("index");
    let direct = view
        .rows
        .iter()
        .find(|row| row.run_key.as_deref() == Some(key.as_str()))
        .expect("the Direct");
    assert_eq!(direct.user_state, Some(RunStateView::Partial));

    // Automatic puts the course's state back.
    let outcome = store::leagues::edit_league(&mut conn, &status(None), UTC).expect("edit");
    assert_eq!(outcome, store::leagues::LeagueEditOutcome::Done);
    assert_eq!(player(&conn).0, None);
}

#[test]
fn mtgo_limited_runs_are_editable_with_picks() {
    let (_dir, conn, now, tz) = tracked("draft_hobbit", "limited-pages-mtgo-flags");
    let view = index(&conn, &index_query(None, None, None), now, tz).expect("index");
    let key = view.rows[0].run_key.clone().expect("a run");
    let event = event(&conn, &key, now, tz)
        .expect("event")
        .expect("the run");
    assert!(event.editable);
    assert!(event.picks_recorded);
}

#[test]
fn an_mtgo_sealed_run_with_a_draft_counts_its_added_booster() {
    let deck = |captured_at_ms: i64, ids: &[u32]| limited::RegisteredDeck {
        match_token: None,
        signature: format!("deck-{captured_at_ms}"),
        captured_at_ms,
        cards: ids
            .iter()
            .map(|catalog_id| RegisteredCard {
                catalog_id: *catalog_id,
                quantity: 1,
                sideboard: false,
            })
            .collect(),
    };
    let mut run = limited_run_for("league:m1", Some("d1"), &[]);
    run.run.kind = LeagueKind::Sealed;
    run.run.format = Some("S6FRA".to_string());
    run.draft = Some(Draft {
        token: "d1".to_string(),
        league_event_id: None,
        course_id: None,
        draft_id: None,
        pod_token: None,
        seat_index: None,
        seat_count: 1,
        booster_catalog_id: None,
        state: DraftState::Finished,
        pack_size: 14,
        picks_expected: 2,
        started_at_ms: Some(T0),
        ended_at_ms: Some(T0),
        picks: vec![Pick {
            ordinal: 1,
            pack_number: 1,
            pick_number: 1,
            pack_id: None,
            direction: None,
            cards_available: vec![1],
            picked_catalog_id: Some(1),
            picked_card_id: None,
            reservations: Vec::new(),
            shown_at_ms: None,
            deadline_at_ms: None,
            picked_at_ms: None,
        }],
    });
    run.registered = vec![deck(T0, &[1, 2]), deck(T0 + HOUR, &[1, 2, 3])];
    // Six boosters, plus the one added after the first registered deck.
    assert_eq!(sealed_packs(&run, &CardFactsMap::new()), Some(7));
}

// Cross-device sync spec section 8: an Arena course's stated record counts
// matches played where MyMTGO never saw them.
#[test]
fn an_arena_run_shows_its_course_record_and_its_untracked_matches() {
    let mut run = limited_run_for("arena:q-course", None, &[]);
    run.course_record = Some((5, 1));
    let row = run_row(&run, &CardFactsMap::new(), &HashMap::new(), chrono_tz::UTC);
    assert_eq!((row.wins, row.losses), (5, 1));
    assert_eq!(
        row.untracked_matches, 6,
        "no match of the run is tracked here"
    );

    // An MTGO run has no course record: its tracked record only.
    let mtgo = limited_run_for("q-league", None, &[]);
    let row = run_row(&mtgo, &CardFactsMap::new(), &HashMap::new(), chrono_tz::UTC);
    assert_eq!((row.wins, row.losses, row.untracked_matches), (0, 0, 0));
}
