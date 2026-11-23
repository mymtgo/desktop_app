//! The home dashboard (dashboard redesign spec section 7, App): every card
//! in each mode, card isolation, the empty window and the JSON the UI reads.

use super::*;
use fixture_tools::TestDir;

const NOW: i64 = 1_790_000_000_000;
const HOUR: i64 = 60 * 60 * 1000;
const UTC: Tz = chrono_tz::UTC;

fn db(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    (dir, conn)
}

/// The account the page shows (the only tracked one).
fn account(conn: &Connection) -> i64 {
    conn.execute(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)",
        [],
    )
    .expect("account");
    conn.last_insert_rowid()
}

/// A deck of `account` with one version.
fn deck(conn: &Connection, account: i64, name: &str, format: &str) -> i64 {
    conn.execute(
        "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
         VALUES (?1, ?2, ?1, ?3, 1, 1, 'file')",
        (name, account, format),
    )
    .expect("deck");
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, 1)",
        (id, format!("sig-{id}")),
    )
    .expect("version");
    id
}

/// A complete match of `account`, a game per result, on `deck_id` when given.
#[allow(clippy::too_many_arguments)]
fn played(
    conn: &Connection,
    account: i64,
    deck_id: Option<i64>,
    token: &str,
    format: &str,
    outcome: &str,
    started_at: i64,
    games: &[&str],
) {
    conn.execute(
        "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
             ended_at, last_activity_at, revision, derived_version)
         VALUES (?1, ?2, 'log', ?3, 'complete', ?4, ?5, ?6, ?6, 1, 1)",
        (
            token,
            account,
            format,
            outcome,
            started_at,
            started_at + HOUR / 2,
        ),
    )
    .expect("match");
    let match_id = conn.last_insert_rowid();
    for (number, result) in games.iter().enumerate() {
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play)
             VALUES (?1, ?2, ?3, 'win', 1)",
            (match_id, number as i64 + 1, *result),
        )
        .expect("game");
    }
    if let Some(deck_id) = deck_id {
        conn.execute(
            "UPDATE matches SET deck_version_id =
                 (SELECT id FROM deck_versions WHERE deck_id = ?2) WHERE token = ?1",
            (token, deck_id),
        )
        .expect("link");
    }
}

fn archetype(conn: &Connection, name: &str) -> i64 {
    conn.execute(
        "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
         VALUES ('q-' || ?1, ?1, 'modern', 'manual', 0, 0)",
        [name],
    )
    .expect("archetype");
    conn.last_insert_rowid()
}

fn faced(conn: &Connection, token: &str, archetype_id: i64) {
    conn.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
         SELECT id, 1, ?2, 1.0 FROM matches WHERE token = ?1",
        (token, archetype_id),
    )
    .expect("seat");
}

/// A complete manual constructed league of `deck_id` holding `tokens`.
fn constructed_league(
    conn: &Connection,
    id: i64,
    account: i64,
    deck_id: i64,
    started_at: i64,
    tokens: &[&str],
) {
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, name, format,
             deck_id, started_at, created_at, token)
         VALUES (?1, 'manual:' || ?1, ?2, 'manual', 'constructed', 'complete', 'Qleague',
             'CMODERN', ?3, ?4, ?4, 'manual_Q' || ?1)",
        (id, account, deck_id, started_at),
    )
    .expect("league");
    for token in tokens {
        conn.execute(
            "UPDATE matches SET league_id = ?1, league_linked = 1 WHERE token = ?2",
            (id, *token),
        )
        .expect("member");
    }
}

/// A QNW draft run in `state` holding `tokens`, with a finished draft that
/// picked `picks` in order.
fn draft_run(
    conn: &Connection,
    id: i64,
    account: i64,
    state: &str,
    started_at: i64,
    tokens: &[&str],
    picks: &[u32],
) {
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name, format,
             set_code, started_at, created_at)
         VALUES (?1, 'q-draft:' || ?1, ?2, 'log', 'draft', ?3, 'q-draft-token-' || ?1,
             'Q Draft League', 'DQNWQNWQNW', 'QNW', ?4, ?4)",
        (id, account, state, started_at),
    )
    .expect("draft run");
    for token in tokens {
        conn.execute(
            "UPDATE matches SET league_id = ?1 WHERE token = ?2",
            (id, *token),
        )
        .expect("member");
    }
    conn.execute(
        "INSERT INTO drafts (id, draft_token, account_id, source, league_id, state, started_at)
         VALUES (?1, 'q-draft-' || ?1, ?2, 'log', ?1, 'finished', ?3)",
        (id, account, started_at),
    )
    .expect("draft");
    for (index, catalog_id) in picks.iter().enumerate() {
        let ordinal = index as i64 + 1;
        conn.execute(
            "INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, picked_catalog_id, reservations)
             VALUES (?1, ?2, 1, ?2, '[]', ?3, '[]')",
            (id, ordinal, *catalog_id),
        )
        .expect("pick");
    }
}

/// The deck registered for `token`: `(catalog id, quantity, sideboard)`.
fn registered(conn: &Connection, token: &str, cards: &[(u32, u32, bool)]) {
    let rows: Vec<serde_json::Value> = cards
        .iter()
        .map(|(id, quantity, side)| {
            serde_json::json!({ "catalog_id": id, "quantity": quantity, "sideboard": side })
        })
        .collect();
    conn.execute(
        "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
         SELECT id, 'imported', ?2, '', started_at FROM matches WHERE token = ?1",
        (token, serde_json::Value::Array(rows).to_string()),
    )
    .expect("registered");
}

fn qnw_cards(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO cards (catalog_id, name, type, rarity, colors, set_code, set_name, art_crop)
         VALUES (101, 'Qbolt', 'Instant', 'rare', 'R', 'QNW', 'Q New Set', 'https://q/101.jpg'),
                (102, 'Qcounter', 'Instant', 'common', 'U', 'QNW', 'Q New Set', NULL),
                (103, 'Qmountain', 'Basic Land', 'common', NULL, 'QNW', 'Q New Set', NULL);",
    )
    .expect("cards");
}

/// One account: Qburn (Modern) won q-m1 and lost q-m2 against Qeldrazi in a
/// complete manual league, then won q-d1 in an Active QNW draft run whose
/// deck is blue and red.
fn seed(conn: &Connection) -> i64 {
    let me = account(conn);
    let burn = deck(conn, me, "Qburn", "CMODERN");
    played(
        conn,
        me,
        Some(burn),
        "q-m1",
        "CMODERN",
        "win",
        NOW - 3 * HOUR,
        &["win", "win"],
    );
    played(
        conn,
        me,
        Some(burn),
        "q-m2",
        "CModern",
        "loss",
        NOW - 2 * HOUR,
        &["loss", "win", "loss"],
    );
    let eldrazi = archetype(conn, "Qeldrazi");
    faced(conn, "q-m1", eldrazi);
    faced(conn, "q-m2", eldrazi);
    constructed_league(conn, 1, me, burn, NOW - 3 * HOUR, &["q-m1", "q-m2"]);
    qnw_cards(conn);
    played(
        conn,
        me,
        None,
        "q-d1",
        "DQNWQNWQNW",
        "win",
        NOW - HOUR,
        &["win", "win"],
    );
    registered(
        conn,
        "q-d1",
        &[(101, 9, false), (102, 8, false), (103, 17, false)],
    );
    draft_run(conn, 2, me, "active", NOW - HOUR, &["q-d1"], &[101, 102]);
    me
}

fn view(conn: &Connection, timeframe: &str, format: &str) -> HomeDashboardView {
    home_dashboard_view(conn, timeframe, format, NOW, UTC).expect("home dashboard")
}

/// The bars' kinds as the UI receives them.
fn kinds(view: &HomeDashboardView) -> Vec<String> {
    let mut kinds = Vec::new();
    for run in &view.league_results.as_ref().expect("league results").runs {
        let json = serde_json::to_value(run.kind).expect("kind json");
        kinds.push(json.as_str().expect("kind text").to_string());
    }
    kinds
}

#[test]
fn all_mode_reads_both_gathers_and_every_card() {
    let (_dir, conn) = db("home-all");
    seed(&conn);
    let all = view(&conn, "week", "all");
    assert!(all.has_matches);
    assert_eq!(all.format, "all");
    assert_eq!(
        all.formats,
        vec![FormatSwitchOptionView {
            key: "modern".to_string(),
            label: "Modern".to_string(),
        }]
    );
    let results = all.results.as_ref().expect("results");
    assert_eq!(results.matches, 3);
    assert_eq!((results.record.wins, results.record.losses), (2, 1));
    assert_eq!(
        results.recent_sessions.len(),
        1,
        "all three matches are today"
    );
    assert_eq!(results.recent_sessions[0].days_ago, 0);
    assert_eq!(results.recent_sessions[0].record.total, 3);
    assert!(!results.daily.is_empty());
    assert_eq!(kinds(&all), vec!["constructed", "draft"], "oldest first");
    let league_runs = all.league_results.as_ref().expect("league results");
    assert_eq!(league_runs.axis_rounds, None, "All mode has no wins axis");
    assert_eq!(league_runs.runs[0].tone, RunBarToneView::Losing);
    assert_eq!(league_runs.runs[1].tone, RunBarToneView::Active);
    let matchups = all.matchups.as_ref().expect("matchups");
    assert_eq!(matchups.len(), 1);
    assert_eq!(
        (matchups[0].name.as_str(), matchups[0].share),
        ("Qeldrazi", 100)
    );
    assert_eq!(all.your_colours, None, "Limited only");
    let recent_matches = all.recent_matches.as_ref().expect("recent matches");
    assert_eq!(recent_matches.len(), 3);
    assert_eq!(recent_matches[0].summary.token, "q-d1");
    assert_eq!(recent_matches[0].set_code.as_deref(), Some("QNW"));
    assert_eq!(
        recent_matches[0].colors,
        Some(vec!["U".to_string(), "R".to_string()])
    );
    assert_eq!(recent_matches[2].deck_name.as_deref(), Some("Qburn"));
    // The Leagues page's run card, for a limited run too.
    let league = all.league.as_ref().expect("league");
    assert_eq!(league.key, "q-draft:2", "the Active run");
    assert!(league.limited);
    assert_eq!(league.name, "QNW Draft");
    assert_eq!(league.format, "Draft");
    assert_eq!(league.format_code.as_deref(), Some("DQNWQNWQNW"));
    assert_eq!(league.state, RunStateView::Active);
    assert_eq!(league.source, crate::leagues::RunSource::Log);
    assert!(!league.manual);
    assert_eq!(league.deck, None);
    assert_eq!(league.version_label, None);
    assert_eq!(league.colors, Some(vec!["U".to_string(), "R".to_string()]));
    assert_eq!(
        league.results,
        vec![Some(crate::leagues::MarkView::W), None, None],
        "an Active run is padded to its three rounds"
    );
    assert_eq!(league.matches.len(), 1);
    assert_eq!((league.game_wins, league.game_losses), (2, 0));
    let recent = all.recent.as_ref().expect("recent");
    assert_eq!(recent.len(), 2);
    assert!(
        matches!(&recent[0], RecentEntryView::Draft(draft) if draft.first_pick.as_deref() == Some("Qbolt"))
    );
    assert!(matches!(&recent[1], RecentEntryView::Deck(deck) if deck.name == "Qburn"));
    assert_eq!(all.most_picked, None, "Limited only");
    assert_eq!(all.last_session.as_ref().expect("session").matches.len(), 3);
    let breakdown = all.breakdown.as_ref().expect("formats");
    assert_eq!(breakdown.kind, "formats");
    assert_eq!(breakdown.total, 3);
    let labels: Vec<&str> = breakdown
        .rows
        .iter()
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(labels, vec!["Modern", "Limited"]);
}

#[test]
fn limited_mode_swaps_the_cards() {
    let (_dir, conn) = db("home-limited");
    seed(&conn);
    let limited = view(&conn, "week", "limited");
    assert_eq!(limited.results.as_ref().expect("results").matches, 1);
    assert_eq!(kinds(&limited), vec!["draft"]);
    assert_eq!(
        limited
            .league_results
            .as_ref()
            .expect("league results")
            .axis_rounds,
        Some(3)
    );
    assert_eq!(limited.matchups, None);
    let colours = limited.your_colours.as_ref().expect("your colours");
    assert_eq!(colours.len(), 1);
    assert_eq!(
        (colours[0].name.as_str(), colours[0].runs, colours[0].share),
        ("Izzet", 1, 100)
    );
    let recent = limited.recent.as_ref().expect("recent drafts");
    assert!(
        recent
            .iter()
            .all(|entry| matches!(entry, RecentEntryView::Draft(_)))
    );
    let picked = limited.most_picked.as_ref().expect("most picked");
    assert_eq!(picked.sets.len(), 1);
    assert_eq!(picked.picks.set_code.as_deref(), Some("QNW"));
    assert_eq!(picked.picks.picks.len(), 2);
    assert_eq!(
        limited
            .last_session
            .as_ref()
            .expect("session")
            .matches
            .len(),
        1
    );
    assert_eq!(limited.breakdown, None, "one set is hidden");
    let league = limited.league.as_ref().expect("league");
    assert_eq!(league.key, "q-draft:2");
    assert!(league.limited);
}

#[test]
fn constructed_mode_reads_no_limited_run() {
    let (_dir, conn) = db("home-constructed");
    seed(&conn);
    let constructed = view(&conn, "week", "constructed");
    assert_eq!(kinds(&constructed), vec!["constructed"]);
    assert_eq!(
        constructed
            .league_results
            .as_ref()
            .expect("league results")
            .axis_rounds,
        Some(5)
    );
    assert_eq!(constructed.your_colours, None);
    assert_eq!(constructed.most_picked, None);
    let recent = constructed.recent.as_ref().expect("recent decks");
    assert_eq!(recent.len(), 1);
    assert!(matches!(&recent[0], RecentEntryView::Deck(_)));
    let league = constructed.league.as_ref().expect("league");
    assert_eq!(league.key, "manual:1");
    assert!(!league.limited);
    assert_eq!(
        league.colors, None,
        "a constructed run's colours are its deck's"
    );
    assert_eq!(league.format, "Modern");
    assert_eq!(league.deck.as_ref().expect("deck").name, "Qburn");
    assert_eq!(league.matches.len(), 2);
    assert_eq!(constructed.breakdown, None, "one format is hidden");
}

#[test]
fn an_active_constructed_run_shows_as_live() {
    let (_dir, conn) = db("home-constructed-active");
    seed(&conn);
    // Only a log run can be Active (a manual league is always complete).
    conn.execute(
        "UPDATE leagues SET source = 'log', run_key = 'q-run:1', state = 'active' WHERE id = 1",
        (),
    )
    .expect("active");
    let constructed = view(&conn, "week", "constructed");
    let league = constructed.league.as_ref().expect("league");
    assert_eq!(league.key, "q-run:1");
    assert_eq!(league.state, RunStateView::Active);
    assert_eq!(league.live_round, Some(3));
}

// Final review finding 6: the League results bars show the player's
// status, as the Leagues page does; the league card keeps the fold's.
#[test]
fn the_league_results_bars_show_the_players_status() {
    let (_dir, conn) = db("home-bars-user-state");
    seed(&conn);
    conn.execute("UPDATE leagues SET user_state = 'partial' WHERE id = 1", ())
        .expect("set by the player");
    let constructed = view(&conn, "week", "constructed");
    let bars = constructed.league_results.as_ref().expect("league results");
    assert_eq!(bars.runs[0].state, RunStateView::Partial);
    assert_eq!(bars.runs[0].tone, RunBarToneView::Partial);
    assert_eq!(bars.trophies, 0);
    assert_eq!(bars.avg_wins, None, "a Partial run is not Complete");
}

#[test]
fn a_format_key_follows_every_code_of_its_label() {
    let (_dir, conn) = db("home-modern");
    seed(&conn);
    let modern = view(&conn, "week", "modern");
    assert_eq!(modern.format, "modern");
    assert_eq!(modern.results.as_ref().expect("results").matches, 2);
    assert_eq!(kinds(&modern), vec!["constructed"]);
}

#[test]
fn an_empty_or_unknown_format_reads_as_the_format_last_played() {
    let (_dir, conn) = db("home-unknown-format");
    let me = seed(&conn);
    let last =
        numbers::last_played_format(&conn, Some(me), store::Platform::Mtgo).expect("last played");
    for key in ["", "legacy"] {
        let opened = view(&conn, "week", key);
        assert_eq!(opened.format, last, "{key:?}");
        assert_ne!(opened.format, "all");
    }
    assert_eq!(view(&conn, "week", "all").format, "all");
    assert_eq!(last, "limited", "the seed newest match is a draft match");
}

#[test]
fn an_empty_window_hides_the_window_cards_and_keeps_the_standing_ones() {
    let (_dir, conn) = db("home-empty-window");
    seed(&conn);
    let empty = view(&conn, "2020-01-01..2020-01-07", "all");
    assert_eq!(empty.results.as_ref().expect("results").matches, 0);
    assert_eq!(empty.league_results, None);
    assert_eq!(empty.matchups, None);
    assert_eq!(empty.recent_matches, None);
    assert_eq!(empty.breakdown, None);
    assert!(empty.league.is_some());
    assert!(empty.recent.is_some());
    assert!(empty.last_session.is_some());
    assert_eq!(
        empty
            .results
            .as_ref()
            .expect("results")
            .recent_sessions
            .len(),
        1,
        "Recent sessions ignore the timeframe"
    );
}

#[test]
fn a_failing_card_read_hides_only_its_cards() {
    let (_dir, conn) = db("home-isolation");
    seed(&conn);
    // The limited gather reads the registered decks: it fails, nothing else does.
    conn.execute_batch("PRAGMA foreign_keys = OFF; DROP TABLE limited_deck_snapshots;")
        .expect("drop");
    let broken = view(&conn, "week", "all");
    assert!(broken.results.is_some());
    assert!(broken.matchups.is_some());
    assert_eq!(broken.league_results, None);
    assert_eq!(broken.league, None);
    let recent = broken.recent.as_ref().expect("recent decks");
    assert_eq!(recent.len(), 1, "the decks still show");
}

#[test]
fn results_count_draws_as_played() {
    let (_dir, conn) = db("home-draws");
    let me = account(&conn);
    let burn = deck(&conn, me, "Qburn", "CMODERN");
    for (index, outcome) in ["win", "loss", "draw", "draw"].iter().enumerate() {
        let game = if *outcome == "win" { "win" } else { "loss" };
        played(
            &conn,
            me,
            Some(burn),
            &format!("m{index}"),
            "CMODERN",
            outcome,
            NOW - HOUR * (index as i64 + 1),
            &[game],
        );
    }
    let all = view(&conn, "alltime", "all");
    let record = &all.results.as_ref().expect("results").record;
    assert_eq!(
        (record.wins, record.losses, record.draws, record.win_rate),
        (1, 1, 2, 25),
        "1 of 4 matches played"
    );
}

#[test]
fn the_chart_trend_reads_matches_before_the_window_and_sessions_ignore_the_window() {
    let (_dir, conn) = db("home-trend");
    let me = account(&conn);
    let burn = deck(&conn, me, "Qburn", "CMODERN");
    let day = 24 * HOUR;
    played(
        &conn,
        me,
        Some(burn),
        "old",
        "CMODERN",
        "loss",
        NOW - 40 * day,
        &["loss"],
    );
    played(
        &conn,
        me,
        Some(burn),
        "new",
        "CMODERN",
        "win",
        NOW - HOUR,
        &["win"],
    );
    let week = view(&conn, "week", "all");
    let results = week.results.as_ref().expect("results");
    assert_eq!(results.matches, 1, "the old loss is outside the week");
    assert_eq!(results.daily.len(), 1);
    // A loss then a win: 1 / (1 + 49/51) = 0.51, not 100.
    assert_eq!(results.daily[0].trend, Some(51));
    let ago: Vec<u32> = results.recent_sessions.iter().map(|s| s.days_ago).collect();
    assert_eq!(ago, vec![0, 40]);
    assert_eq!(results.recent_sessions[1].record.label, "0 - 1");
}

fn entry(key: &str, state: RunState, started_at: i64) -> DashboardLeagueRun {
    DashboardLeagueRun {
        key: key.to_string(),
        kind: LeagueKind::Constructed,
        rounds: 5,
        wins: 0,
        losses: 0,
        state,
        shown_state: state,
        name: String::new(),
        subject: String::new(),
        started_at,
        last_played_at: started_at,
        format: None,
        deck_id: None,
        set_code: None,
        set_name: None,
        colors: None,
        colour_name: None,
        cover_art: None,
        local_cover_art: None,
        game_wins: 0,
        game_losses: 0,
        first_pick: None,
        match_tokens: Vec::new(),
    }
}

#[test]
fn the_league_card_is_the_run_in_progress_else_the_newest() {
    let runs = vec![
        entry("new", RunState::Complete, 3),
        entry("live", RunState::Active, 2),
        entry("old", RunState::Dropped, 1),
    ];
    assert_eq!(
        league_card_run(&runs).map(|run| run.key.as_str()),
        Some("live")
    );
    let finished = vec![
        entry("new", RunState::Partial, 3),
        entry("old", RunState::Complete, 1),
    ];
    assert_eq!(
        league_card_run(&finished).map(|run| run.key.as_str()),
        Some("new")
    );
    assert_eq!(league_card_run(&[]), None);
    // A run the player marked Active is not the run in progress: the card
    // reads the fold's state.
    let marked = vec![
        DashboardLeagueRun {
            shown_state: RunState::Active,
            ..entry("marked", RunState::Complete, 3)
        },
        entry("live", RunState::Active, 2),
    ];
    assert_eq!(
        league_card_run(&marked).map(|run| run.key.as_str()),
        Some("live")
    );
}

#[test]
fn the_ui_sees_tagged_recent_entries_and_camel_case() {
    let (_dir, conn) = db("home-json");
    seed(&conn);
    let json = serde_json::to_value(view(&conn, "week", "all")).expect("json");
    assert_eq!(json["recent"][0]["type"], "draft");
    assert_eq!(json["recent"][0]["firstPick"], "Qbolt");
    assert_eq!(json["recent"][1]["type"], "deck");
    assert_eq!(json["leagueResults"]["runs"][0]["tone"], "losing");
    assert_eq!(json["leagueResults"]["runs"][0]["state"], "complete");
    assert_eq!(json["breakdown"]["kind"], "formats");
    assert_eq!(json["recentMatches"][0]["match"]["token"], "q-d1");
    assert_eq!(json["hasMatches"], true);
    assert_eq!(json["league"]["key"], "q-draft:2");
    assert_eq!(json["league"]["limited"], true);
    assert_eq!(json["league"]["formatCode"], "DQNWQNWQNW");
    assert_eq!(json["league"]["colors"][0], "U");
}

#[test]
fn the_arena_dashboard_has_both_rank_lines() {
    let (_dir, conn) = db("home-arena-rank");
    crate::rank::tests::seed(&conn);
    let view = view(&conn, "alltime", "");
    let lines = view.rank.expect("the rank card");
    let ladders: Vec<crate::rank::LadderView> = lines.iter().map(|line| line.ladder).collect();
    assert_eq!(
        ladders,
        vec![
            crate::rank::LadderView::Constructed,
            crate::rank::LadderView::Limited
        ]
    );
    assert_eq!(lines[0].current.as_deref(), Some("Platinum 4"));
    assert!(lines[0].gaps.iter().all(|gap| !gap.of_deck));
    assert_eq!(lines[1].current.as_deref(), Some("Silver 2"));
    assert_eq!(lines[1].gaps.len(), 1, "limited is drawn between readings");
}

#[test]
fn an_arena_account_with_no_readings_still_has_both_rank_cards() {
    let (_dir, conn) = db("home-arena-no-rank");
    crate::rank::tests::arena_account(&conn);
    let lines = view(&conn, "alltime", "").rank.expect("the rank card");
    let ladders: Vec<crate::rank::LadderView> = lines.iter().map(|line| line.ladder).collect();
    assert_eq!(
        ladders,
        vec![
            crate::rank::LadderView::Constructed,
            crate::rank::LadderView::Limited
        ]
    );
    for line in &lines {
        assert_eq!(line.current, None);
        assert!(line.points.is_empty() && line.gaps.is_empty() && line.matches.is_empty());
    }
}

#[test]
fn the_mtgo_dashboard_has_no_rank() {
    let (_dir, conn) = db("home-mtgo-rank");
    seed(&conn);
    assert_eq!(view(&conn, "alltime", "").rank, None);
}
