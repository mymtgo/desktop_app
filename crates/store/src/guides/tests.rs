//! Guide and note reads (0.x `GetSideboardGuideSummariesTest`,
//! `GetArchetypeNotesTest`, the store side of `BuildSideboardGuideTest`)
//! and the table CHECKs. The fixture helpers are shared with the edit
//! tests.

use super::*;
use crate::archetypes::tests::insert_archetype;
use crate::test_support::number;
use decks::DeckCard;
use fixture_tools::TestDir;
use rusqlite::params;

pub(crate) fn db(label: &str) -> (TestDir, Connection) {
    crate::archetypes::tests::db(label)
}

/// A Modern deck named `name`.
pub(crate) fn deck(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
         VALUES ('Q-deck-' || (SELECT COUNT(*) FROM decks), ?1, 'CMODERN', 1, 1, 'file')
         RETURNING id",
        [name],
        |row| row.get(0),
    )
    .expect("deck")
}

/// A version of the deck holding `cards` as `(catalog_id, quantity,
/// sideboard)`, newer than every version before it.
pub(crate) fn version(conn: &Connection, deck_id: i64, cards: &[(u32, u32, bool)]) -> i64 {
    let list: Vec<DeckCard> = cards
        .iter()
        .map(|(catalog_id, quantity, sideboard)| DeckCard {
            catalog_id: *catalog_id,
            quantity: *quantity,
            sideboard: *sideboard,
        })
        .collect();
    conn.query_row(
        "INSERT INTO deck_versions (deck_id, signature, modified_at)
         VALUES (?1, ?2, (SELECT COUNT(*) + 1 FROM deck_versions)) RETURNING id",
        params![deck_id, decks::signature(&list)],
        |row| row.get(0),
    )
    .expect("version")
}

pub(crate) fn card(conn: &Connection, catalog_id: u32, oracle: &str, name: &str, card_type: &str) {
    conn.execute(
        "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
         VALUES (?1, ?2, ?3, ?4, 'W')",
        params![catalog_id, oracle, name, card_type],
    )
    .expect("card");
}

/// 0.x `BuildSideboardGuideTest`'s cards: Rest in Peace (201), Lightning
/// Bolt (202 and 302), Cut Down (203).
pub(crate) fn guide_cards_on_file(conn: &Connection) {
    card(conn, 201, "o-rip", "Rest in Peace", "Enchantment");
    card(conn, 202, "o-bolt", "Lightning Bolt", "Instant");
    card(conn, 203, "o-cut", "Cut Down", "Instant");
    card(conn, 302, "o-bolt", "Lightning Bolt", "Instant");
}

pub(crate) fn archetype(conn: &Connection, name: &str) -> i64 {
    insert_archetype(
        conn,
        &format!("q-{name}"),
        name,
        Some("modern"),
        "downloaded",
    )
}

pub(crate) fn guide(conn: &Connection, deck_id: i64, archetype_id: i64) -> i64 {
    conn.query_row(
        "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
         VALUES (?1, ?2, 1, 1) RETURNING id",
        [deck_id, archetype_id],
        |row| row.get(0),
    )
    .expect("guide")
}

pub(crate) fn plan(conn: &Connection, guide_id: i64, oracle: &str, direction: &str, quantity: u32) {
    conn.execute(
        "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
         VALUES (?1, ?2, ?3, ?4)",
        params![guide_id, oracle, direction, quantity],
    )
    .expect("plan card");
}

pub(crate) fn note(
    conn: &Connection,
    deck_id: i64,
    archetype_id: i64,
    body: &str,
    created_at: i64,
) -> i64 {
    conn.query_row(
        "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
        params![deck_id, archetype_id, body, created_at],
        |row| row.get(0),
    )
    .expect("note")
}

/// A complete match of `version_id` against `archetype_id` (the opponent
/// seat) with `outcome` and one game per result (game 1 first).
pub(crate) fn played(
    conn: &Connection,
    version_id: i64,
    archetype_id: i64,
    outcome: &str,
    results: &[&str],
) -> (i64, Vec<i64>) {
    let match_id: i64 = conn
        .query_row(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, last_activity_at, revision, derived_version,
                 deck_version_id)
             VALUES ('Q-m-' || (SELECT COUNT(*) FROM matches), 1, 1, 'log', 'CMODERN', 'League',
                 'complete', ?1, 1000, 1000, 1, 0, ?2)
             RETURNING id",
            params![outcome, version_id],
            |row| row.get(0),
        )
        .expect("match");
    conn.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         VALUES (?1, 1, ?2, 1.0, 0)",
        [match_id, archetype_id],
    )
    .expect("opponent seat");
    let mut games = Vec::new();
    for (index, result) in results.iter().enumerate() {
        let game_id: i64 = conn
            .query_row(
                "INSERT INTO games (match_id, game_number, result, end_reason)
                 VALUES (?1, ?2, ?3, 'win') RETURNING id",
                params![match_id, index as i64 + 1, result],
                |row| row.get(0),
            )
            .expect("game");
        games.push(game_id);
    }
    (match_id, games)
}

/// 0.x `guideGame`: a match whose postboard game (game 2) has the given
/// local rows `(catalog_id, sided_in, sided_out)`.
pub(crate) fn guide_game(
    conn: &Connection,
    version_id: i64,
    archetype_id: i64,
    won: bool,
    rows: &[(u32, bool, bool)],
) {
    let result = if won { "win" } else { "loss" };
    let (_, games) = played(conn, version_id, archetype_id, result, &["win", result]);
    for (catalog_id, sided_in, sided_out) in rows {
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty, sided_in, sided_out)
             VALUES (?1, 'local', ?2, 2, 0, ?3, ?4)",
            params![games[1], catalog_id, u32::from(*sided_in) * 2, u32::from(*sided_out) * 2],
        )
        .expect("game card");
    }
}

// ---- GetSideboardGuideSummariesTest -----------------------------------------

#[test]
fn summarises_each_guide_with_card_totals_note_count_and_matchup_record() {
    let (_dir, conn) = db("guides-summary");
    let deck_id = deck(&conn, "Qdeck");
    let v = version(&conn, deck_id, &[]);
    let blink = archetype(&conn, "Esper Blink");
    conn.execute(
        "UPDATE archetypes SET color_identity = 'WUB' WHERE id = ?1",
        [blink],
    )
    .expect("colours");
    let g = guide(&conn, deck_id, blink);
    plan(&conn, g, "o-a", "in", 2);
    plan(&conn, g, "o-b", "in", 1);
    plan(&conn, g, "o-c", "out", 3);
    note(&conn, deck_id, blink, "Qnote one", 1);
    note(&conn, deck_id, blink, "Qnote two", 2);
    played(&conn, v, blink, "win", &["win"]);
    played(&conn, v, blink, "win", &["win"]);
    played(&conn, v, blink, "loss", &["loss"]);

    let summaries = deck_guides(&conn, deck_id).expect("guides");
    assert_eq!(summaries.len(), 1);
    let summary = &summaries[0];
    assert_eq!(summary.id, g);
    assert_eq!(summary.archetype_name, "Esper Blink");
    assert_eq!(summary.archetype_color_identity.as_deref(), Some("W,U,B"));
    assert_eq!((summary.cards_in, summary.cards_out), (3, 3));
    assert_eq!(summary.notes_count, 2);
    assert_eq!(
        summary.matchup,
        Matchup {
            matches: 3,
            match_record: Some("2 - 1".to_string()),
            match_winrate: Some(67),
            game_winrate: Some(67),
        }
    );
}

#[test]
fn returns_null_win_rates_for_an_archetype_the_deck_has_never_faced() {
    let (_dir, conn) = db("guides-never-faced");
    let deck_id = deck(&conn, "Qdeck");
    let tron = archetype(&conn, "Mono Green Tron");
    guide(&conn, deck_id, tron);

    let summary = deck_guides(&conn, deck_id).expect("guides").remove(0);
    assert_eq!(summary.matchup, Matchup::default());
    assert_eq!(summary.matchup.match_record, None);
    assert_eq!((summary.cards_in, summary.notes_count), (0, 0));
}

#[test]
fn orders_by_archetype_name_and_ignores_other_decks() {
    let (_dir, conn) = db("guides-order");
    let deck_id = deck(&conn, "Qdeck");
    let other = deck(&conn, "Qother");
    let tron = archetype(&conn, "Tron");
    let blink = archetype(&conn, "Blink");
    guide(&conn, deck_id, tron);
    guide(&conn, deck_id, blink);
    guide(&conn, other, blink);

    let names: Vec<String> = deck_guides(&conn, deck_id)
        .expect("guides")
        .into_iter()
        .map(|summary| summary.archetype_name)
        .collect();
    assert_eq!(names, ["Blink", "Tron"]);
}

#[test]
fn summarises_a_single_guide() {
    let (_dir, conn) = db("guides-one");
    let deck_id = deck(&conn, "Qdeck");
    let g = guide(&conn, deck_id, archetype(&conn, "Qarch"));
    assert_eq!(
        guide_summary(&conn, g).expect("read").map(|s| s.id),
        Some(g)
    );
    assert_eq!(guide_summary(&conn, g + 1).expect("read"), None);
}

/// A draw is played but neither won nor lost (0.x `MatchRecord`).
#[test]
fn a_drawn_match_shows_in_the_record() {
    let (_dir, conn) = db("guides-draw");
    let deck_id = deck(&conn, "Qdeck");
    let v = version(&conn, deck_id, &[]);
    let a = archetype(&conn, "Qarch");
    guide(&conn, deck_id, a);
    played(&conn, v, a, "win", &["win"]);
    played(&conn, v, a, "draw", &["draw"]);
    // A match without games is not counted (0.x joins the game stats).
    played(&conn, v, a, "win", &[]);

    let summary = deck_guides(&conn, deck_id).expect("guides").remove(0);
    assert_eq!(summary.matchup.matches, 2);
    assert_eq!(summary.matchup.match_record.as_deref(), Some("1 - 0 - 1"));
    assert_eq!(summary.matchup.match_winrate, Some(50));
    assert_eq!(summary.matchup.game_winrate, Some(100));
}

/// The guide numbers are the deck Matchups tab's (T9's `matchup_spread`
/// over every version, all time): two versions, two archetypes, a draw, an
/// undecided game, a match without games, a hidden match and an
/// unfinished one.
#[test]
fn the_guide_numbers_are_the_matchups_tab_numbers() {
    let (_dir, conn) = db("guides-matchups-tab");
    let deck_id = deck(&conn, "Qdeck");
    let old = version(&conn, deck_id, &[(201, 1, false)]);
    let new = version(&conn, deck_id, &[(202, 1, false)]);
    let a = archetype(&conn, "Qalpha");
    let b = archetype(&conn, "Qbeta");
    guide(&conn, deck_id, a);
    guide(&conn, deck_id, b);
    played(&conn, old, a, "win", &["win", "loss", "win"]);
    played(&conn, new, a, "loss", &["loss", "loss"]);
    played(&conn, new, a, "draw", &["win", "loss", "draw"]);
    played(&conn, new, a, "win", &[]);
    let (hidden, _) = played(&conn, new, a, "win", &["win"]);
    conn.execute("UPDATE matches SET hidden_at = 1 WHERE id = ?1", [hidden])
        .expect("hide");
    let (running, _) = played(&conn, new, a, "win", &["win"]);
    conn.execute(
        "UPDATE matches SET state = 'in_progress' WHERE id = ?1",
        [running],
    )
    .expect("unfinished");
    played(&conn, old, b, "win", &["win", "unknown"]);

    let summaries = deck_guides(&conn, deck_id).expect("guides");
    let alpha = &summaries[0].matchup;
    let beta = &summaries[1].matchup;
    assert_eq!(
        *alpha,
        Matchup {
            matches: 3,
            match_record: Some("1 - 1 - 1".to_string()),
            match_winrate: Some(33),
            game_winrate: Some(43),
        }
    );
    assert_eq!(
        *beta,
        Matchup {
            matches: 1,
            match_record: Some("1 - 0".to_string()),
            match_winrate: Some(100),
            game_winrate: Some(100),
        }
    );

    let versions = crate::matchups::deck_version_ids(&conn, deck_id, None).expect("versions");
    let spread =
        crate::matchups::matchup_spread(&conn, &versions, decks::stats::Range::all_time(), None)
            .expect("spread");
    for (archetype_id, matchup) in [(a, alpha), (b, beta)] {
        let row = spread
            .iter()
            .find(|row| row.archetype_id == archetype_id)
            .expect("row");
        assert_eq!(row.record.total, matchup.matches);
        assert_eq!(Some(row.record.win_rate), matchup.match_winrate);
        assert_eq!(Some(row.games.win_rate), matchup.game_winrate);
    }
}

// ---- BuildSideboardGuideTest (the store's half) -----------------------------

#[test]
fn reports_sided_in_w_l_and_the_postboard_baseline_against_the_archetype() {
    let (_dir, conn) = db("guides-editor-stats");
    guide_cards_on_file(&conn);
    let deck_id = deck(&conn, "Qdeck");
    let a = archetype(&conn, "Esper Blink");
    // 201 sideboard, 202 maindeck, 203 sideboard.
    let v = version(
        &conn,
        deck_id,
        &[(201, 2, true), (202, 4, false), (203, 2, true)],
    );
    guide_game(&conn, v, a, true, &[(201, true, false), (202, false, true)]);
    guide_game(
        &conn,
        v,
        a,
        false,
        &[(201, true, false), (202, false, true)],
    );
    let g = guide(&conn, deck_id, a);

    let page = guide_editor(&conn, g).expect("read").expect("guide");
    assert!(page.has_version);
    assert_eq!(page.sideboard_size, 4);
    let editor = page.editor.expect("editor");
    assert_eq!(editor.postboard_games, 2);
    assert_eq!(editor.postboard_record, "1 - 1");
    let rip = editor
        .bring_in
        .iter()
        .find(|row| row.oracle_id == "o-rip")
        .expect("rip");
    assert_eq!(
        (rip.sided_in_games, rip.wins, rip.losses, rip.winrate),
        (2, 1, 1, Some(50))
    );
    assert_eq!(rip.name, "Rest in Peace");
    assert_eq!(rip.catalog_id, Some(201));
    let cut = editor
        .bring_in
        .iter()
        .find(|row| row.oracle_id == "o-cut")
        .expect("cut");
    assert_eq!((cut.sided_in_games, cut.winrate), (0, None));
    assert_eq!(editor.take_out.len(), 1);
    assert_eq!(editor.take_out[0].oracle_id, "o-bolt");
    assert_eq!(editor.take_out[0].sided_out_games, 2);
}

#[test]
fn lists_cards_from_the_current_version_while_counting_games_from_every_version() {
    let (_dir, conn) = db("guides-editor-versions");
    guide_cards_on_file(&conn);
    let deck_id = deck(&conn, "Qdeck");
    let a = archetype(&conn, "Esper Blink");
    let old = version(&conn, deck_id, &[(201, 2, true), (203, 2, true)]);
    // Cut Down (203) is gone from the current list.
    version(&conn, deck_id, &[(201, 2, true), (202, 4, false)]);
    guide_game(
        &conn,
        old,
        a,
        true,
        &[(201, true, false), (203, true, false)],
    );
    let g = guide(&conn, deck_id, a);

    let editor = guide_editor(&conn, g)
        .expect("read")
        .expect("guide")
        .editor
        .expect("editor");
    let oracles: Vec<&str> = editor
        .bring_in
        .iter()
        .map(|row| row.oracle_id.as_str())
        .collect();
    assert_eq!(oracles, ["o-rip"]);
    assert_eq!(editor.bring_in[0].sided_in_games, 1);
}

#[test]
fn excludes_games_against_other_archetypes() {
    let (_dir, conn) = db("guides-editor-other");
    guide_cards_on_file(&conn);
    let deck_id = deck(&conn, "Qdeck");
    let a = archetype(&conn, "Esper Blink");
    let burn = archetype(&conn, "Burn");
    let v = version(&conn, deck_id, &[(201, 2, true)]);
    guide_game(&conn, v, burn, true, &[(201, true, false)]);
    let g = guide(&conn, deck_id, a);

    let editor = guide_editor(&conn, g)
        .expect("read")
        .expect("guide")
        .editor
        .expect("editor");
    assert_eq!(editor.postboard_games, 0);
    assert_eq!(editor.bring_in[0].sided_in_games, 0);
}

/// 0.x `EditController` without a version: nothing to plan with, the notes
/// still work.
#[test]
fn a_deck_without_a_version_has_no_editor_but_its_notes() {
    let (_dir, conn) = db("guides-editor-no-version");
    let deck_id = deck(&conn, "Qdeck");
    let a = archetype(&conn, "Qarch");
    let g = guide(&conn, deck_id, a);
    note(&conn, deck_id, a, "Qmull aggressively", 5);

    let page = guide_editor(&conn, g).expect("read").expect("guide");
    assert!(!page.has_version);
    assert_eq!(page.editor, None);
    assert_eq!(page.notes.current.len(), 1);
    assert_eq!(guide_editor(&conn, g + 1).expect("read"), None);
}

// ---- GetArchetypeNotesTest ----------------------------------------------------

#[test]
fn splits_notes_into_this_deck_and_other_decks_newest_first() {
    let (_dir, conn) = db("guides-notes");
    let a = archetype(&conn, "Esper Blink");
    let other_archetype = archetype(&conn, "Burn");
    let deck_id = deck(&conn, "My Qmurktide");
    let other_deck = deck(&conn, "My Qburn");
    let older = note(&conn, deck_id, a, "Older note", 1_000);
    let newer = note(&conn, deck_id, a, "Newer note", 2_000);
    note(&conn, other_deck, a, "From the other deck", 1_500);
    note(&conn, deck_id, other_archetype, "Different matchup", 3_000);

    let notes = archetype_notes(&conn, deck_id, a).expect("notes");
    let ids: Vec<i64> = notes.current.iter().map(|note| note.id).collect();
    assert_eq!(ids, [newer, older]);
    assert_eq!(notes.other.len(), 1);
    assert_eq!(notes.other[0].body, "From the other deck");
    assert_eq!(notes.other[0].deck_name, "My Qburn");
}

#[test]
fn returns_empty_groups_when_the_archetype_has_no_notes() {
    let (_dir, conn) = db("guides-notes-none");
    let deck_id = deck(&conn, "Qdeck");
    let a = archetype(&conn, "Qarch");
    assert_eq!(
        archetype_notes(&conn, deck_id, a).expect("notes"),
        ArchetypeNotes::default()
    );
}

// ---- The archetype page's block -------------------------------------------------

#[test]
fn the_archetype_block_lists_each_deck_with_a_guide_or_notes() {
    let (_dir, conn) = db("guides-archetype-block");
    guide_cards_on_file(&conn);
    let a = archetype(&conn, "Qarch");
    let other = archetype(&conn, "Qother");
    let zeta = deck(&conn, "Qzeta");
    let alpha = deck(&conn, "Qalpha");
    let archived = deck(&conn, "Qarchived");
    let unrelated = deck(&conn, "Qunrelated");
    conn.execute("UPDATE decks SET archived_at = 5 WHERE id = ?1", [archived])
        .expect("archive");
    let g = guide(&conn, zeta, a);
    plan(&conn, g, "o-rip", "in", 2);
    plan(&conn, g, "o-cut", "in", 1);
    plan(&conn, g, "o-bolt", "out", 3);
    plan(&conn, g, "o-unknown", "out", 1);
    note(&conn, alpha, a, "Qalpha note", 10);
    note(&conn, alpha, a, "Qalpha newer", 20);
    guide(&conn, archived, a);
    guide(&conn, unrelated, other);
    let v = version(&conn, zeta, &[]);
    played(&conn, v, a, "win", &["win", "win"]);

    let rows = archetype_guides(&conn, Platform::Mtgo, a).expect("block");
    let decks: Vec<&str> = rows.iter().map(|row| row.deck_name.as_str()).collect();
    assert_eq!(decks, ["Qalpha", "Qzeta", "Qarchived"]);

    let alpha_row = &rows[0];
    assert_eq!(alpha_row.guide_id, None);
    let bodies: Vec<&str> = alpha_row.notes.iter().map(|n| n.body.as_str()).collect();
    assert_eq!(bodies, ["Qalpha newer", "Qalpha note"]);

    let zeta_row = &rows[1];
    assert_eq!(zeta_row.guide_id, Some(g));
    assert_eq!((zeta_row.cards_in, zeta_row.cards_out), (3, 4));
    let names: Vec<(&str, u32)> = zeta_row
        .planned_in
        .iter()
        .map(|card| (card.name.as_str(), card.quantity))
        .collect();
    assert_eq!(names, [("Cut Down", 1), ("Rest in Peace", 2)]);
    let names: Vec<&str> = zeta_row
        .planned_out
        .iter()
        .map(|card| card.name.as_str())
        .collect();
    assert_eq!(names, ["Lightning Bolt", "Unknown card"]);
    assert_eq!(zeta_row.matchup.match_record.as_deref(), Some("1 - 0"));
    assert!(rows[2].deck_archived);
}

// ---- The CHECKs ---------------------------------------------------------------

#[test]
fn the_tables_refuse_rows_outside_their_checks() {
    let (_dir, conn) = db("guides-checks");
    let deck_id = deck(&conn, "Qdeck");
    let a = archetype(&conn, "Qarch");
    let g = guide(&conn, deck_id, a);
    let insert_card = |oracle: &str, direction: &str, quantity: i64| {
        conn.execute(
            "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
             VALUES (?1, ?2, ?3, ?4)",
            params![g, oracle, direction, quantity],
        )
    };
    assert!(insert_card("o-a", "in", 0).is_err(), "quantity 0");
    assert!(insert_card("o-a", "in", 100).is_err(), "quantity 100");
    assert!(insert_card("o-a", "sideways", 1).is_err(), "direction");
    assert!(insert_card("", "in", 1).is_err(), "empty oracle id");
    assert!(
        insert_card(&"o".repeat(65), "in", 1).is_err(),
        "long oracle id"
    );
    assert!(insert_card("o-a", "in", 99).is_ok());
    assert!(insert_card("o-a", "out", 1).is_ok(), "both directions");
    assert!(
        insert_card("o-a", "in", 1).is_err(),
        "one row per direction"
    );

    let insert_note = |body: &str| {
        conn.execute(
            "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
             VALUES (?1, ?2, ?3, 1)",
            params![deck_id, a, body],
        )
    };
    assert!(insert_note("").is_err(), "empty body");
    assert!(insert_note(&"a".repeat(2001)).is_err(), "body too long");
    assert!(
        insert_note(&"é".repeat(2000)).is_ok(),
        "characters, not bytes"
    );

    assert!(
        conn.execute(
            "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
             VALUES (?1, ?2, 1, 1)",
            [deck_id, a],
        )
        .is_err(),
        "one guide per deck and archetype"
    );
    assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 1);
}

// ---- The archetype page's deck select (spec section 21.4) ---------------------

/// A deck of `format` (an MTGO code) and `account`.
fn deck_of(conn: &Connection, name: &str, format: &str, account: Option<i64>) -> i64 {
    let id = deck(conn, name);
    conn.execute(
        "UPDATE decks SET format = ?1, account_id = ?2 WHERE id = ?3",
        params![format, account, id],
    )
    .expect("deck format");
    id
}

fn account_named(conn: &Connection, name: &str) -> i64 {
    conn.query_row(
        "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0) RETURNING id",
        [name],
        |row| row.get(0),
    )
    .expect("account")
}

#[test]
fn the_deck_select_offers_the_formats_decks_most_played_against_it_first() {
    let (_dir, conn) = db("guides-deck-options");
    let me = account_named(&conn, "Qme");
    let other = account_named(&conn, "Qother");
    let a = archetype(&conn, "Qarch");
    let rarely = deck_of(&conn, "Qrarely", "CMODERN", Some(me));
    let often = deck_of(&conn, "Qoften", "CMODERN", Some(me));
    let never = deck_of(&conn, "Qnever", "CMODERN", Some(me));
    let also_never = deck_of(&conn, "Qanother", "CMODERN", Some(me));
    let legacy = deck_of(&conn, "Qlegacy", "CLEGACY", Some(me));
    let archived = deck_of(&conn, "Qarchived", "CMODERN", Some(me));
    conn.execute("UPDATE decks SET archived_at = 1 WHERE id = ?1", [archived])
        .expect("archive");
    let theirs = deck_of(&conn, "Qtheirs", "CMODERN", Some(other));
    let v_rarely = version(&conn, rarely, &[(201, 1, false)]);
    let v_often = version(&conn, often, &[(202, 1, false)]);
    let v_legacy = version(&conn, legacy, &[(203, 1, false)]);
    played(&conn, v_rarely, a, "win", &["win"]);
    played(&conn, v_often, a, "win", &["win"]);
    played(&conn, v_often, a, "loss", &["loss"]);
    played(&conn, v_legacy, a, "loss", &["loss"]);

    let options = guide_deck_options(&conn, Some(me), Platform::Mtgo, a).expect("options");
    let ids: Vec<i64> = options.iter().map(|option| option.deck_id).collect();
    assert_eq!(ids, [often, rarely, also_never, never]);
    assert_eq!(options[0].name, "Qoften");
    assert_eq!(options[0].matches, 2);
    assert_eq!(options[2].matches, 0);
    let _ = theirs;

    // Without a current account, every account's decks.
    let every = guide_deck_options(&conn, None, Platform::Mtgo, a).expect("options");
    assert_eq!(every.len(), 5);
    // An unknown archetype offers nothing.
    assert!(
        guide_deck_options(&conn, Some(me), Platform::Mtgo, a + 100)
            .expect("options")
            .is_empty()
    );
}

/// Homebrew and Rogue fit every format: every constructed deck is offered.
#[test]
fn a_fallback_offers_every_constructed_deck() {
    let (_dir, conn) = db("guides-deck-options-fallback");
    let homebrew: i64 = conn
        .query_row(
            "SELECT id FROM archetypes WHERE name = 'Homebrew'",
            [],
            |row| row.get(0),
        )
        .expect("homebrew");
    let modern = deck_of(&conn, "Qmodern", "CMODERN", None);
    let legacy = deck_of(&conn, "Qlegacy", "CLEGACY", None);
    deck_of(&conn, "Qdraft", "DMKM", None);

    let ids: Vec<i64> = guide_deck_options(&conn, None, Platform::Mtgo, homebrew)
        .expect("options")
        .iter()
        .map(|option| option.deck_id)
        .collect();
    assert_eq!(ids, [legacy, modern]);
}

#[test]
fn the_deck_select_reads_one_platform() {
    let (_dir, conn) = db("guides-deck-options-arena");
    let a = archetype(&conn, "Qarch");
    let mtgo = deck_of(&conn, "Qmtgo", "CMODERN", None);
    let arena = deck_of(&conn, "Qarena", "CMODERN", None);
    crate::test_support::move_deck_to_arena(&conn, arena);

    let ids: Vec<i64> = guide_deck_options(&conn, None, Platform::Mtgo, a)
        .expect("options")
        .iter()
        .map(|option| option.deck_id)
        .collect();
    assert_eq!(ids, [mtgo]);
    let ids: Vec<i64> = guide_deck_options(&conn, None, Platform::Arena, a)
        .expect("options")
        .iter()
        .map(|option| option.deck_id)
        .collect();
    assert_eq!(ids, [arena]);
}

#[test]
fn archetype_guides_scope_by_platform() {
    let (_dir, conn) = db("guides-archetype-platform");
    guide_cards_on_file(&conn);
    let a = archetype(&conn, "Qarch");
    let mtgo = deck_of(&conn, "Qmtgo", "CMODERN", None);
    let arena = deck_of(&conn, "Qarena", "CMODERN", None);
    crate::test_support::move_deck_to_arena(&conn, arena);
    guide(&conn, mtgo, a);
    guide(&conn, arena, a);
    note(&conn, arena, a, "Qarena note", 10);

    let names = |platform| -> Vec<String> {
        archetype_guides(&conn, platform, a)
            .expect("block")
            .into_iter()
            .map(|row| row.deck_name)
            .collect()
    };
    assert_eq!(names(Platform::Mtgo), ["Qmtgo"]);
    assert_eq!(names(Platform::Arena), ["Qarena"]);
}
