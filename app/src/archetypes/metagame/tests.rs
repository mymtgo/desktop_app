use super::*;
use crate::api::session::testing::device_api;
use crate::api::stub::{Reply, Stub};
use crate::archetypes::cache::METAGAME_TTL_MS;
use crate::archetypes::definitions::Clock;
use fixture_tools::TestDir;
use mymtgo_api::{DecklistCard, GlobalCard, GlobalShare};
use store::params;

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;
const UTC: chrono_tz::Tz = chrono_tz::UTC;

/// Never waits: the pacing is the worker's business.
struct NoWait;

impl Clock for NoWait {
    fn now_ms(&self) -> i64 {
        NOW
    }
    fn sleep_ms(&self, _: i64) {}
}

fn pacer() -> Pacer {
    Pacer::new(Box::new(NoWait))
}

/// One archetype of the website's field, without tile cards.
fn global_row(uuid: &str, share: Option<f64>) -> GlobalShare {
    GlobalShare {
        uuid: uuid.to_string(),
        name: "Qname".to_string(),
        color_identity: None,
        share,
        matches: 1,
        cards: Vec::new(),
    }
}

fn shares(rows: &[(&str, Option<f64>)]) -> MetagameShares {
    MetagameShares {
        format: "modern".to_string(),
        days: 30,
        window_start: "2027-01-01".to_string(),
        window_end: "2027-01-30".to_string(),
        matches: 100,
        archetypes: rows
            .iter()
            .map(|(uuid, share)| global_row(uuid, *share))
            .collect(),
        other_share: None,
        unnameable_share: None,
    }
}

// Spec section 9: offline the global column is blank and nothing is asked.
#[test]
fn offline_leaves_the_global_column_blank_without_a_request() {
    let mut cache = TimedCache::new(METAGAME_TTL_MS);
    let (answer, fetch) = global_answer(true, "modern", 30, &mut cache, NOW);
    assert_eq!(answer, GlobalAnswer::Unavailable);
    assert_eq!(fetch, None);
    assert_eq!(cache.get("modern:30", NOW), Cached::Missing);

    // The worker's fetch sends nothing offline either.
    let stub = Stub::start(|_| Reply::json(200, "{}"));
    let offline = device_api(&stub.base_url, true);
    assert!(fetch_global(&offline.state, &mut pacer(), "modern", 30).is_err());
    assert!(fetch_live_decklists(&offline.state, &mut pacer(), "q-uuid").is_err());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(stub.connections(), 0);
}

#[test]
fn the_global_column_is_fetched_once_then_read_from_the_cache() {
    let mut cache = TimedCache::new(METAGAME_TTL_MS);
    // A format the website does not list.
    assert_eq!(
        global_answer(false, "commander", 30, &mut cache, NOW),
        (GlobalAnswer::Unavailable, None)
    );
    let (answer, fetch) = global_answer(false, "modern", 30, &mut cache, NOW);
    assert_eq!(answer, GlobalAnswer::Loading);
    assert_eq!(fetch, Some(("modern".to_string(), 30)));
    // Asked once: the next read waits for it.
    assert_eq!(
        global_answer(false, "modern", 30, &mut cache, NOW),
        (GlobalAnswer::Loading, None)
    );
    cache.store("modern:30", Some(shares(&[("q-storm", Some(6.2))])), NOW);
    assert_eq!(
        global_answer(false, "modern", 30, &mut cache, NOW + 1),
        (
            GlobalAnswer::Ready(vec![global_row("q-storm", Some(6.2))]),
            None
        )
    );
    // A failure reads as unavailable.
    cache.begin("modern:7");
    cache.store("modern:7", None, NOW);
    assert_eq!(
        global_answer(false, "modern", 7, &mut cache, NOW),
        (GlobalAnswer::Unavailable, None)
    );
}

#[test]
fn the_worker_reads_the_shares_from_the_api() {
    let stub = Stub::start(|request| {
        if request.target == "/api/metagame/modern?days=30" {
            return Reply::json(
                200,
                r#"{"format":"modern","days":30,"window_start":"2027-01-01","window_end":"2027-01-30",
                    "matches":10,"archetypes":[{"uuid":"q-storm","name":"Qstorm","color_identity":"R",
                    "share":6.2,"matches":1}],"other_share":1.0,"unnameable_share":2.0}"#,
            );
        }
        Reply::json(404, r#"{"message":"Unknown format."}"#)
    });
    let online = device_api(&stub.base_url, false);
    let read = fetch_global(&online.state, &mut pacer(), "modern", 30).expect("shares");
    assert_eq!(read.archetypes[0].uuid, "q-storm");
    assert_eq!(read.archetypes[0].share, Some(6.2));
    // The endpoint missing (A3 not live) is a failure: the column stays blank.
    assert!(fetch_global(&online.state, &mut pacer(), "legacy", 30).is_err());
}

fn db(dir: &TestDir) -> Connection {
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    conn.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);
         INSERT INTO archetypes (id, uuid, name, format, color_identity, source, created_at, updated_at)
             VALUES (10, 'q-storm', 'Qstorm', 'modern', 'U,R', 'downloaded', 0, 0),
                    (11, 'q-burn', 'Qburn', 'modern', 'R', 'manual', 0, 0);",
    )
    .expect("rows");
    conn
}

fn add_match(conn: &Connection, id: i64, outcome: &str, opponent: Option<i64>) {
    conn.execute(
        "INSERT INTO matches (id, token, account_id, opponent_id, source, format, match_type,
             state, outcome, started_at, last_activity_at, revision, derived_version)
         VALUES (?1, 'Q-match-' || ?1, 1, 1, 'log', 'CMODERN', 'Modern', 'complete', ?2,
             ?3, ?3, 1, 0)",
        params![id, outcome, NOW - DAY],
    )
    .expect("match");
    if let Some(archetype) = opponent {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, 1, ?2, 1.0, 0)",
            params![id, archetype],
        )
        .expect("seat");
    }
}

/// Name, share, global share, difference, record and colours.
type TileRead<'a> = (&'a str, f64, Option<f64>, Option<f64>, &'a str, Vec<String>);

#[test]
fn the_page_fills_the_tiles_and_asks_for_the_global_column() {
    let dir = TestDir::new("metagame-page");
    let conn = db(&dir);
    add_match(&conn, 1, "win", Some(10));
    add_match(&conn, 2, "loss", Some(10));
    add_match(&conn, 3, "win", Some(11));
    add_match(&conn, 4, "win", None);
    let mut asked = Vec::new();
    let mut global = |key: &str, days: u32| {
        asked.push((key.to_string(), days));
        GlobalAnswer::Ready(vec![
            global_row("q-storm", Some(40.0)),
            global_row("q-burn", Some(10.0)),
        ])
    };
    let page = metagame_page(
        &conn,
        None,
        MetagameWindow::Timeframe(Timeframe::Last7),
        None,
        NOW,
        UTC,
        &mut global,
    )
    .expect("page");
    assert_eq!(asked, vec![("modern".to_string(), 7)]);
    assert_eq!(page.format.as_deref(), Some("modern"));
    assert_eq!(page.format_label.as_deref(), Some("Modern"));
    assert_eq!(page.timeframe_label, "Last 7 days");
    assert_eq!(page.account.as_deref(), Some("Qlocal"));
    assert_eq!(page.total_matches, 4);
    assert_eq!(page.without_archetype, 1);
    assert_eq!(page.global_state, GlobalState::Ready);
    assert!(page.range_from.is_some());
    let read: Vec<TileRead> = page
        .tiles
        .iter()
        .map(|t| {
            (
                t.name.as_str(),
                t.share,
                t.global_share,
                t.difference,
                t.record.as_str(),
                t.colors.clone(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (
                "Qstorm",
                50.0,
                Some(40.0),
                Some(10.0),
                "1 - 1",
                vec!["U".to_string(), "R".to_string()]
            ),
            ("Qburn", 25.0, None, None, "1 - 0", vec!["R".to_string()]),
        ]
    );
    // A format never played falls back to the most played.
    let mut loading = |_: &str, _: u32| GlobalAnswer::Loading;
    let page = metagame_page(
        &conn,
        Some("legacy"),
        MetagameWindow::Timeframe(Timeframe::AllTime),
        None,
        NOW,
        UTC,
        &mut loading,
    )
    .expect("page");
    assert_eq!(page.format.as_deref(), Some("modern"));
    assert_eq!(page.global_state, GlobalState::Loading);
    assert_eq!(page.range_from, None);
}

// A tile shows the website tile's cards; without them (a manual archetype
// here, or the shares not here yet) it picks from its stored list.
#[test]
fn tiles_show_the_websites_cards_else_their_stored_lists() {
    let dir = TestDir::new("metagame-tile-cards");
    let conn = db(&dir);
    add_match(&conn, 1, "win", Some(10));
    add_match(&conn, 2, "win", Some(11));
    conn.execute_batch(
        "INSERT INTO cards (catalog_id, name, type, image, art_crop) VALUES
             (501, 'Qbolt', 'Instant', 'https://cards.scryfall.io/b.jpg', 'https://cards.scryfall.io/b-art.jpg'),
             (502, 'Qmountain', 'Basic Land', 'https://cards.scryfall.io/m.jpg', NULL),
             (503, 'Qhelix', 'Instant', 'https://cards.scryfall.io/h.jpg', NULL);
         UPDATE cards SET local_image = '/q/501.jpg', local_art_crop = '/q/501_art.jpg' WHERE catalog_id = 501;
         INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard) VALUES
             (10, 1, 503, 4, 0),
             (11, 1, 501, 3, 0),
             (11, 1, 502, 18, 0),
             (11, 1, 503, 4, 0);",
    )
    .expect("lists");
    let website = GlobalCard {
        name: "Qstormcard".to_string(),
        image: "https://cards.scryfall.io/s.jpg".to_string(),
        art: Some("https://cards.scryfall.io/s-art.jpg".to_string()),
    };
    let mut storm = global_row("q-storm", Some(40.0));
    storm.cards = vec![website];
    let mut ready = |_: &str, _: u32| GlobalAnswer::Ready(vec![storm.clone()]);
    let page = metagame_page(
        &conn,
        None,
        MetagameWindow::Timeframe(Timeframe::Last7),
        None,
        NOW,
        UTC,
        &mut ready,
    )
    .expect("page");
    let cards = |name: &str| -> Vec<TileCardView> {
        page.tiles
            .iter()
            .find(|tile| tile.name == name)
            .expect(name)
            .cards
            .clone()
    };
    assert_eq!(
        cards("Qstorm"),
        vec![TileCardView {
            name: "Qstormcard".to_string(),
            image: Some("https://cards.scryfall.io/s.jpg".to_string()),
            local_image: None,
            art: Some("https://cards.scryfall.io/s-art.jpg".to_string()),
            local_art: None,
        }]
    );
    assert_eq!(
        cards("Qburn"),
        vec![
            TileCardView {
                name: "Qhelix".to_string(),
                image: Some("https://cards.scryfall.io/h.jpg".to_string()),
                local_image: None,
                art: None,
                local_art: None,
            },
            TileCardView {
                name: "Qbolt".to_string(),
                image: Some("https://cards.scryfall.io/b.jpg".to_string()),
                local_image: Some("/q/501.jpg".to_string()),
                art: Some("https://cards.scryfall.io/b-art.jpg".to_string()),
                local_art: Some("/q/501_art.jpg".to_string()),
            },
        ]
    );
    // While the shares load, the downloaded archetype shows its key list.
    let mut loading = |_: &str, _: u32| GlobalAnswer::Loading;
    let page = metagame_page(
        &conn,
        None,
        MetagameWindow::Timeframe(Timeframe::Last7),
        None,
        NOW,
        UTC,
        &mut loading,
    )
    .expect("page");
    let storm = page
        .tiles
        .iter()
        .find(|tile| tile.name == "Qstorm")
        .expect("storm");
    let names: Vec<&str> = storm.cards.iter().map(|card| card.name.as_str()).collect();
    assert_eq!(names, vec!["Qhelix"]);
}

#[test]
fn a_page_without_matches_has_no_format() {
    let dir = TestDir::new("metagame-empty");
    let conn = db(&dir);
    let mut global = |_: &str, _: u32| -> GlobalAnswer { panic!("not asked without a format") };
    let page = metagame_page(
        &conn,
        None,
        MetagameWindow::Timeframe(Timeframe::Last30),
        None,
        NOW,
        UTC,
        &mut global,
    )
    .expect("page");
    assert_eq!(page.format, None);
    assert!(page.tiles.is_empty());
    assert_eq!(page.global_state, GlobalState::Unavailable);
}

// Review Focus 4: the MTGO page has no months or bands, and a month or a
// rank band given to it changes nothing but the (default) timeframe.
#[test]
fn the_mtgo_page_is_unchanged() {
    let dir = TestDir::new("metagame-mtgo-unchanged");
    let conn = db(&dir);
    add_match(&conn, 1, "win", Some(10));
    add_match(&conn, 2, "loss", Some(10));
    add_match(&conn, 3, "win", Some(11));
    add_match(&conn, 4, "win", None);
    let mut asked = Vec::new();
    let mut global = |key: &str, days: u32| {
        asked.push((key.to_string(), days));
        GlobalAnswer::Ready(vec![global_row("q-storm", Some(40.0))])
    };
    let page = metagame_page(
        &conn,
        None,
        MetagameWindow::Timeframe(Timeframe::Last30),
        None,
        NOW,
        UTC,
        &mut global,
    )
    .expect("page");
    assert!(page.months.is_empty());
    assert!(page.bands.is_empty());
    assert_eq!(page.month, None);
    assert_eq!(page.band, None);
    assert_eq!(page.total_matches, 4);
    assert_eq!(page.timeframe, "last30");
    let ignored = metagame_page(
        &conn,
        None,
        MetagameWindow::Month {
            year: 2020,
            month: 1,
        },
        Some(RankBand::Gold),
        NOW,
        UTC,
        &mut global,
    )
    .expect("ignored");
    assert_eq!(ignored, page);
    assert_eq!(
        asked,
        vec![("modern".to_string(), 30), ("modern".to_string(), 30)]
    );
}

// A month only applies on Arena: the MTGO page keeps the asked timeframe.
#[test]
fn the_mtgo_page_with_a_month_reads_the_requested_timeframe() {
    let dir = TestDir::new("metagame-mtgo-month");
    let conn = db(&dir);
    add_match(&conn, 1, "win", Some(10));
    let window = page_window(&conn, "last7", Some("2020-01")).expect("window");
    assert_eq!(window, MetagameWindow::Timeframe(Timeframe::Last7));
    let mut global = |_: &str, _: u32| GlobalAnswer::Unavailable;
    let page = metagame_page(&conn, None, window, None, NOW, UTC, &mut global).expect("page");
    assert_eq!(page.timeframe, "last7");
    assert_eq!(page.total_matches, 1);
    // On Arena the month is used; a bad one gives the timeframe (the
    // page then falls back to the current month).
    let arena_dir = TestDir::new("metagame-arena-window");
    let arena = arena_db(&arena_dir);
    assert_eq!(
        page_window(&arena, "last7", Some("2026-11")).expect("arena"),
        MetagameWindow::Month {
            year: 2026,
            month: 11
        }
    );
    assert_eq!(
        page_window(&arena, "last7", Some("soon")).expect("bad"),
        MetagameWindow::Timeframe(Timeframe::Last7)
    );
}

// ---- Arena -------------------------------------------------------------------

/// 2027-01-15T08:00:00Z is `NOW`: January 2027 is the current month.
const JANUARY_2: i64 = NOW - 13 * DAY;
const DECEMBER_20: i64 = NOW - 26 * DAY;

/// An Arena account (id 2) read by the pages, with one Arena log file.
fn arena_db(dir: &TestDir) -> Connection {
    let conn = db(dir);
    conn.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
             VALUES (2, 'Qarena#12345', 0, 'arena', 'Qarena-id');
         INSERT INTO settings (key, value) VALUES ('platform_context', 'arena');
         INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len, first_seen_at)
             VALUES (1, '/arena/Player.log', 1, 2, 3, 4);",
    )
    .expect("arena");
    conn
}

/// A complete Pioneer Arena match `Q-arena-{id}` of `match_type`, its last
/// log event at `offset` when given.
fn arena_match(
    conn: &Connection,
    id: i64,
    match_type: &str,
    started_at: i64,
    offset: Option<i64>,
    opponent: Option<i64>,
) {
    conn.execute(
        "INSERT INTO matches (id, token, account_id, source, format, match_type, state, outcome,
             started_at, last_activity_at, revision, derived_version)
         VALUES (?1, 'Q-arena-' || ?1, 2, 'log', 'CPIONEER', ?2, 'complete', 'win', ?3, ?3, 1, 0)",
        params![id, match_type, started_at],
    )
    .expect("arena match");
    if let Some(offset) = offset {
        conn.execute(
            "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind, match_token,
                 logged_at, raw, compressed) VALUES (1, ?1, 0, 'game_info', 'Q-arena-' || ?2, 1, X'7B7D', 0)",
            params![offset, id],
        )
        .expect("event");
    }
    if let Some(archetype) = opponent {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, 1, ?2, 1.0, 0)",
            params![id, archetype],
        )
        .expect("seat");
    }
}

/// A constructed rank reading of the Arena account.
fn reading(conn: &Connection, offset: i64, class: &str, observed_at: i64) {
    conn.execute(
        "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at, class,
             level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
         VALUES (2, 'constructed', 95, ?1, ?2, 4, 0, 0, 0, 1, ?3)",
        params![observed_at, class, offset],
    )
    .expect("reading");
}

fn never(_: &str, _: u32) -> GlobalAnswer {
    panic!("the Arena page never asks the global column")
}

fn arena_page(conn: &Connection, window: MetagameWindow, band: Option<RankBand>) -> MetagamePage {
    metagame_page(conn, None, window, band, NOW, UTC, &mut never).expect("arena page")
}

/// January: a Bronze ranked match, a Gold ranked match and a play match.
/// December: one ranked match without a reading.
fn seed_arena(conn: &Connection) {
    reading(conn, 100, "Bronze", JANUARY_2);
    arena_match(conn, 1, "ranked", JANUARY_2, Some(200), Some(10));
    reading(conn, 300, "Gold", JANUARY_2 + 1000);
    arena_match(conn, 2, "ranked", JANUARY_2 + DAY, Some(400), Some(11));
    arena_match(conn, 3, "play", JANUARY_2 + DAY, Some(500), None);
    arena_match(conn, 4, "ranked", DECEMBER_20, Some(50), Some(10));
}

#[test]
fn an_arena_month_counts_that_month_only() {
    let dir = TestDir::new("metagame-arena-month");
    let conn = arena_db(&dir);
    seed_arena(&conn);
    // No month: the current one.
    let page = arena_page(&conn, MetagameWindow::Timeframe(Timeframe::Last30), None);
    assert_eq!(page.format.as_deref(), Some("pioneer"));
    assert_eq!(page.month.as_deref(), Some("2027-01"));
    assert_eq!(page.total_matches, 3);
    assert_eq!(page.range_from.as_deref(), Some("2027-01-01"));
    assert_eq!(page.range_to.as_deref(), Some("2027-01-31"));
    assert_eq!(page.timeframe, "last30");
    let months: Vec<(&str, &str)> = page
        .months
        .iter()
        .map(|m| (m.key.as_str(), m.label.as_str()))
        .collect();
    assert_eq!(
        months,
        vec![("2027-01", "January 2027"), ("2026-12", "December 2026")]
    );

    let december = arena_page(
        &conn,
        MetagameWindow::Month {
            year: 2026,
            month: 12,
        },
        None,
    );
    assert_eq!(december.month.as_deref(), Some("2026-12"));
    assert_eq!(december.total_matches, 1);
    assert_eq!(december.range_from.as_deref(), Some("2026-12-01"));

    // A month without matches of the format falls back to the current one.
    let unknown = arena_page(
        &conn,
        MetagameWindow::Month {
            year: 2025,
            month: 6,
        },
        None,
    );
    assert_eq!(unknown.month.as_deref(), Some("2027-01"));
    assert_eq!(unknown.total_matches, 3);
}

#[test]
fn an_arena_month_lists_the_current_month_without_matches() {
    let dir = TestDir::new("metagame-arena-current");
    let conn = arena_db(&dir);
    arena_match(&conn, 4, "ranked", DECEMBER_20, Some(50), Some(10));
    let page = arena_page(&conn, MetagameWindow::Timeframe(Timeframe::Last30), None);
    let keys: Vec<&str> = page.months.iter().map(|m| m.key.as_str()).collect();
    assert_eq!(keys, vec!["2027-01", "2026-12"]);
    assert_eq!(page.total_matches, 0);
}

#[test]
fn an_arena_rank_tab_counts_its_band() {
    let dir = TestDir::new("metagame-arena-band");
    let conn = arena_db(&dir);
    seed_arena(&conn);
    let january = MetagameWindow::Month {
        year: 2027,
        month: 1,
    };
    let gold = arena_page(&conn, january, Some(RankBand::Gold));
    assert_eq!(gold.band.as_deref(), Some("gold"));
    assert_eq!(gold.total_matches, 1);
    let names: Vec<&str> = gold.tiles.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["Qburn"]);
    let bronze = arena_page(&conn, january, Some(RankBand::Bronze));
    let names: Vec<&str> = bronze.tiles.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["Qstorm"]);
    // The play match has no rank: Unranked, without an archetype.
    let unranked = arena_page(&conn, january, Some(RankBand::Unranked));
    assert_eq!(unranked.total_matches, 1);
    assert_eq!(unranked.without_archetype, 1);
}

// A rank tab without matches that month (a stale link) shows All, as an
// unknown month shows the current one.
#[test]
fn an_empty_rank_tab_falls_back_to_all() {
    let dir = TestDir::new("metagame-arena-empty-band");
    let conn = arena_db(&dir);
    seed_arena(&conn);
    let january = MetagameWindow::Month {
        year: 2027,
        month: 1,
    };
    let mythic = arena_page(&conn, january, Some(RankBand::Mythic));
    let all = arena_page(&conn, january, None);
    assert_eq!(mythic.band, None);
    assert_eq!(mythic.total_matches, 3);
    assert_eq!(mythic, all);
}

#[test]
fn an_arena_page_without_matches_still_lists_the_current_month() {
    let dir = TestDir::new("metagame-arena-no-matches");
    let conn = arena_db(&dir);
    let page = arena_page(&conn, MetagameWindow::Timeframe(Timeframe::Last30), None);
    assert_eq!(page.format, None);
    assert_eq!(page.month.as_deref(), Some("2027-01"));
    let months: Vec<(&str, &str)> = page
        .months
        .iter()
        .map(|m| (m.key.as_str(), m.label.as_str()))
        .collect();
    assert_eq!(months, vec![("2027-01", "January 2027")]);
    assert_eq!(page.range_from.as_deref(), Some("2027-01-01"));
    assert_eq!(page.total_matches, 0);
}

// Review Focus 5: every band is listed, empty ones with 0, and the bands
// add up to the month's total (All).
#[test]
fn an_arena_month_lists_every_band_with_counts() {
    let dir = TestDir::new("metagame-arena-bands");
    let conn = arena_db(&dir);
    seed_arena(&conn);
    let page = arena_page(
        &conn,
        MetagameWindow::Month {
            year: 2027,
            month: 1,
        },
        None,
    );
    assert_eq!(page.band, None);
    let bands: Vec<(&str, &str, u32)> = page
        .bands
        .iter()
        .map(|b| (b.key.as_str(), b.label.as_str(), b.matches))
        .collect();
    assert_eq!(
        bands,
        vec![
            ("spark", "Spark", 0),
            ("bronze", "Bronze", 1),
            ("silver", "Silver", 0),
            ("gold", "Gold", 1),
            ("platinum", "Platinum", 0),
            ("diamond", "Diamond", 0),
            ("mythic", "Mythic", 0),
            ("unranked", "Unranked", 1),
        ]
    );
    let sum: u32 = page.bands.iter().map(|b| b.matches).sum();
    assert_eq!(sum, page.total_matches);
    // December's match had no reading that month: Unranked.
    let december = arena_page(
        &conn,
        MetagameWindow::Month {
            year: 2026,
            month: 12,
        },
        None,
    );
    let unranked = december
        .bands
        .iter()
        .find(|b| b.key == "unranked")
        .expect("unranked");
    assert_eq!(unranked.matches, 1);
}

#[test]
fn the_arena_page_never_asks_the_global_column() {
    let dir = TestDir::new("metagame-arena-global");
    let conn = arena_db(&dir);
    seed_arena(&conn);
    let page = arena_page(&conn, MetagameWindow::Timeframe(Timeframe::Last7), None);
    assert_eq!(page.global_state, GlobalState::Unavailable);
    assert!(page.tiles.iter().all(|t| t.global_share.is_none()));
    let banded = arena_page(
        &conn,
        MetagameWindow::Timeframe(Timeframe::Last7),
        Some(RankBand::Gold),
    );
    assert_eq!(banded.global_state, GlobalState::Unavailable);
}

#[test]
fn a_month_key_parses_and_prints() {
    assert_eq!(parse_month("2026-11"), Some((2026, 11)));
    assert_eq!(parse_month("2026-13"), None);
    assert_eq!(parse_month("2026-1x"), None);
    assert_eq!(parse_month("november"), None);
    assert_eq!(month_key(2026, 3), "2026-03");
    assert_eq!(month_label(2026, 11), "November 2026");
}

fn deck(uuid: &str, seen_count: u32, cards: &[(u32, &str)]) -> ArchetypeDeck {
    ArchetypeDeck {
        uuid: uuid.to_string(),
        seen_count,
        cards: cards
            .iter()
            .map(|(mtgo_id, oracle)| DecklistCard {
                mtgo_id: *mtgo_id,
                oracle_id: (*oracle).to_string(),
                name: Some(format!("Qcard {mtgo_id}")),
                card_type: Some("Instant".to_string()),
                color_identity: Some("R".to_string()),
                image: None,
                quantity: 4,
                sideboard: false,
            })
            .collect(),
    }
}

// 0.x ShowTest "returns multiple decks ordered by seen_count desc" and the
// variant picker's label.
#[test]
fn typical_lists_are_most_seen_first() {
    let lists = typical_lists(&[
        deck("q-low", 2, &[(1, "o-a")]),
        deck("q-high", 10, &[(2, "o-b")]),
    ]);
    let read: Vec<(&str, &str)> = lists
        .iter()
        .map(|list| (list.uuid.as_str(), list.label.as_str()))
        .collect();
    assert_eq!(
        read,
        vec![
            ("q-high", "Variant 1 · seen 10×"),
            ("q-low", "Variant 2 · seen 2×")
        ]
    );
    assert_eq!(lists[0].cards[0].name, "Qcard 2");
    assert_eq!(lists[0].cards[0].quantity, 4);
}

#[test]
fn the_decklists_follow_the_source_offline_mode_and_the_cache() {
    let state = |source, offline, cached| {
        decklists_before_request(source, offline, cached).map(|view| view.state)
    };
    assert_eq!(
        state(Source::Manual, false, Cached::Missing),
        Some(DecklistsState::Manual)
    );
    assert_eq!(
        state(Source::Fallback, false, Cached::Missing),
        Some(DecklistsState::Fallback)
    );
    // Offline: no request, the stored key list instead.
    assert_eq!(
        state(Source::Downloaded, true, Cached::Missing),
        Some(DecklistsState::Offline)
    );
    assert_eq!(
        state(Source::Downloaded, false, Cached::Failed),
        Some(DecklistsState::Unavailable)
    );
    assert_eq!(
        state(
            Source::Downloaded,
            false,
            Cached::Fresh(vec![deck("q", 1, &[])])
        ),
        Some(DecklistsState::Ready)
    );
    assert_eq!(state(Source::Downloaded, false, Cached::Missing), None);
}

#[test]
fn the_detail_marks_cards_seen_against_the_live_decklists() {
    let dir = TestDir::new("metagame-detail");
    let conn = db(&dir);
    add_match(&conn, 1, "win", Some(10));
    conn.execute_batch(
        "INSERT INTO games (id, match_id, game_number, result, end_reason, started_at)
             VALUES (1, 1, 1, 'win', 'win', 0);
         INSERT INTO game_cards (game_id, side, catalog_id, seen) VALUES (1, 'opponent', 500, 2),
             (1, 'opponent', 600, 1);
         INSERT INTO cards (catalog_id, oracle_id, name) VALUES (500, 'o-bolt', 'Qbolt'),
             (600, 'o-tech', 'Qtech');",
    )
    .expect("cards");
    let read = |live: Option<&[ArchetypeDeck]>| {
        let detail = store::metagame::archetype_detail(
            &conn,
            None,
            store::Platform::Mtgo,
            10,
            decks::stats::Range::all_time(),
            UTC,
        )
        .expect("detail")
        .expect("found");
        let view = detail_view(&conn, detail, Timeframe::AllTime, live).expect("view");
        view.cards_seen
            .iter()
            .map(|card| (card.name.clone(), card.in_typical))
            .collect::<Vec<_>>()
    };
    // Without live lists (and no stored list) nothing is typical.
    assert_eq!(
        read(None),
        vec![("Qbolt".to_string(), false), ("Qtech".to_string(), false)]
    );
    let live = vec![deck("q-list", 3, &[(999, "o-bolt")])];
    assert_eq!(
        read(Some(&live)),
        vec![("Qbolt".to_string(), true), ("Qtech".to_string(), false)]
    );
}

// The cards seen grid draws each card: its image and downloaded file from
// the card data; a card without a row has neither.
#[test]
fn cards_seen_carry_their_images() {
    let dir = TestDir::new("metagame-seen-images");
    let conn = db(&dir);
    add_match(&conn, 1, "win", Some(10));
    conn.execute_batch(
        "INSERT INTO games (id, match_id, game_number, result, end_reason, started_at)
             VALUES (1, 1, 1, 'win', 'win', 0);
         INSERT INTO game_cards (game_id, side, catalog_id, seen) VALUES (1, 'opponent', 500, 2),
             (1, 'opponent', 600, 1);
         INSERT INTO cards (catalog_id, name, image, local_image)
             VALUES (500, 'Qbolt', 'https://cards.scryfall.io/b.jpg', '/q/500.jpg');",
    )
    .expect("cards");
    let detail = store::metagame::archetype_detail(
        &conn,
        None,
        store::Platform::Mtgo,
        10,
        decks::stats::Range::all_time(),
        UTC,
    )
    .expect("detail")
    .expect("found");
    let view = detail_view(&conn, detail, Timeframe::AllTime, None).expect("view");
    let images: Vec<(u32, Option<String>, Option<String>)> = view
        .cards_seen
        .iter()
        .map(|card| {
            (
                card.catalog_id,
                card.image.clone(),
                card.local_image.clone(),
            )
        })
        .collect();
    assert_eq!(
        images,
        vec![
            (
                500,
                Some("https://cards.scryfall.io/b.jpg".to_string()),
                Some("/q/500.jpg".to_string())
            ),
            (600, None, None),
        ]
    );
}

#[test]
fn the_saved_image_is_a_png() {
    assert_eq!(image_name("modern-metagame"), "modern-metagame.png");
    assert_eq!(image_name("modern-metagame.png"), "modern-metagame.png");
    assert_eq!(image_name("  "), "metagame.png");
}

// Issue #326: the website-style decklist shows mana costs. The API's
// decklist cards carry none, so the local card data fills them in.
#[test]
fn live_lists_take_mana_costs_and_local_images_from_the_card_data() {
    let mut lists = typical_lists(&[deck("q", 3, &[(1, "o-a"), (2, "o-b")])]);
    let mut known = std::collections::HashMap::new();
    known.insert(
        1,
        store::decks::CardRow {
            catalog_id: 1,
            oracle_id: None,
            name: Some("Qcard 1".to_string()),
            card_type: Some("Creature".to_string()),
            sub_type: None,
            rarity: None,
            color_identity: None,
            colors: None,
            cmc: Some(2.0),
            mana_cost: Some("{1}{R}".to_string()),
            set_name: None,
            set_code: None,
            image: None,
            art_crop: Some("art.jpg".to_string()),
            local_image: Some("1.jpg".to_string()),
            local_art_crop: None,
        },
    );
    with_local_cards(&mut lists, &known);
    let first = &lists[0].cards[0];
    assert_eq!(first.mana_cost.as_deref(), Some("{1}{R}"));
    assert_eq!(first.cmc, Some(2.0));
    assert_eq!(first.local_image.as_deref(), Some("1.jpg"));
    assert_eq!(first.art_crop.as_deref(), Some("art.jpg"));
    let second = &lists[0].cards[1];
    assert_eq!(second.mana_cost, None);
    assert_eq!(second.local_image, None);
}

// Review of #326: a card the API cannot name takes its name, type and
// colours from the local card data, so it is grouped and previewed.
#[test]
fn live_cards_the_api_cannot_name_take_the_local_name_type_and_colours() {
    let mut unnamed = deck("q", 3, &[(5, "o-e")]);
    unnamed.cards[0].name = None;
    unnamed.cards[0].card_type = None;
    unnamed.cards[0].color_identity = None;
    let mut lists = typical_lists(&[unnamed]);
    assert!(!lists[0].cards[0].known);
    let mut known = std::collections::HashMap::new();
    known.insert(
        5,
        store::decks::CardRow {
            catalog_id: 5,
            oracle_id: None,
            name: Some("Qlocal".to_string()),
            card_type: Some("Creature".to_string()),
            sub_type: None,
            rarity: None,
            color_identity: Some("G".to_string()),
            colors: None,
            cmc: Some(1.0),
            mana_cost: Some("{G}".to_string()),
            set_name: None,
            set_code: None,
            image: Some("5.jpg".to_string()),
            art_crop: None,
            local_image: None,
            local_art_crop: None,
        },
    );
    with_local_cards(&mut lists, &known);
    let card = &lists[0].cards[0];
    assert_eq!(card.name, "Qlocal");
    assert_eq!(card.type_line.as_deref(), Some("Creature"));
    assert_eq!(card.colors, vec!["G".to_string()]);
    assert!(card.known);
    assert_eq!(card.image.as_deref(), Some("5.jpg"));
}

// Issue #326 (owner, 2026-10-11): the Decklist tab showed no mana costs for
// cards the user never met, since only local card data carries them. Those
// cards are queued for the card lookup; once it stores them, the decklist
// reads their costs.
#[test]
fn decklist_cards_without_card_data_are_looked_up_and_then_show_mana_costs() {
    let stub = Stub::start(|request| {
        let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap_or_default();
        let answers: Vec<serde_json::Value> = body["ids"]
            .as_array()
            .map(|ids| {
                ids.iter()
                    .filter_map(serde_json::Value::as_i64)
                    .collect::<Vec<i64>>()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|id| {
                serde_json::json!({"value": id, "scryfall_id": format!("scryfall-{id}"),
                    "oracle_id": format!("oracle-{id}"), "name": format!("Qcard {id}"),
                    "mana_cost": "{2}{U}", "cmc": 3,
                    "image": "https://example.test/card.png"})
            })
            .collect();
        Reply::json(200, &serde_json::Value::Array(answers).to_string())
    });
    let api = device_api(&stub.base_url, false);
    let run = crate::cards::populate::tests::tracked("decklist-costs");
    run.write(
        "INSERT INTO cards (catalog_id, scryfall_id, name, mana_cost)
             VALUES (101, 'scryfall-101', 'Qcard 101', '{R}');",
    );
    let view = || ArchetypeDecklistsView {
        state: DecklistsState::Ready,
        lists: typical_lists(&[deck(
            "q",
            3,
            &[(101, "o-a"), (202, "o-b"), (2_000_000, "o-c"), (202, "o-b")],
        )]),
    };
    let reader = || store::open_reader(&run.state.db_path).expect("reader");

    let mut asked: Vec<Vec<u32>> = Vec::new();
    queue_unknown_cards(&reader(), &view(), &mut |ids| asked.push(ids)).expect("queued");
    assert_eq!(asked, vec![vec![202]], "only the card without a row, once");

    let mut before = view();
    fill_local_cards(&reader(), &mut before).expect("filled");
    assert_eq!(before.lists[0].cards[0].mana_cost.as_deref(), Some("{R}"));
    assert_eq!(before.lists[0].cards[1].mana_cost, None);

    // The queue's path: a stub through the tracker, then a population run.
    let scanner = lock(&run.state.tracker)
        .as_ref()
        .expect("tracker")
        .card_stub_scanner();
    assert_eq!(scanner.ensure(asked.remove(0)), Ok(1));
    run.populate(&api.state, None).expect("populated");

    let mut after = view();
    fill_local_cards(&reader(), &mut after).expect("filled");
    assert_eq!(after.lists[0].cards[1].mana_cost.as_deref(), Some("{2}{U}"));
    assert_eq!(after.lists[0].cards[1].cmc, Some(3.0));

    let mut again: Vec<Vec<u32>> = Vec::new();
    queue_unknown_cards(&reader(), &view(), &mut |ids| again.push(ids)).expect("queued");
    assert!(again.is_empty(), "stored cards are not asked about again");
}
