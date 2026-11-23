//! v1 projection vs 0.x goldens. Every difference must be fixed or justified in
//! fixtures/parity/allowlist.json. Stale allowlist entries also fail.
//!
//! Only the log fixtures' goldens are projection goldens: allowlist entries
//! for other fixtures (`match_report`, pinned by `mymtgo-api`'s report tests)
//! are not this test's.

use std::collections::BTreeSet;

use fixture_tools::{fixtures_dir, load_fixture, prepared_events};
use mtgo_match::{GameResult, MatchState, MatchView, Outcome, Projector};
use serde::Deserialize;

#[derive(Deserialize)]
struct Golden {
    fixture: String,
    matches: Vec<GoldenMatch>,
}

#[derive(Deserialize)]
struct GoldenMatch {
    token: String,
    state: String,
    outcome: Option<String>,
    games_won: Option<u32>,
    games_lost: Option<u32>,
    format: Option<String>,
    match_type: Option<String>,
    opponent: Option<String>,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    games: Vec<GoldenGame>,
}

#[derive(Deserialize)]
struct GoldenGame {
    mtgo_id: u64,
    won: Option<bool>,
    local_on_play: Option<bool>,
    local_mulligans: Option<u32>,
    opp_mulligans: Option<u32>,
    local_die_roll: Option<u32>,
    opp_die_roll: Option<u32>,
}

#[derive(Deserialize)]
struct Allowed {
    fixture: String,
    token: String,
    field: String,
    #[allow(dead_code)]
    reason: String,
}

type DiffKey = (String, String, String);

fn state_name(state: MatchState) -> &'static str {
    match state {
        MatchState::InProgress => "in_progress",
        MatchState::Ended => "ended",
        MatchState::Complete => "complete",
        MatchState::Abandoned => "abandoned",
    }
}

fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Win => "win",
        Outcome::Loss => "loss",
        Outcome::Draw => "draw",
        Outcome::Unknown => "unknown",
    }
}

fn unknown_as_none(value: &Option<String>) -> Option<String> {
    value.clone().filter(|v| v != "Unknown")
}

fn compare_match(
    golden: &GoldenMatch,
    view: &MatchView,
    diffs: &mut Vec<(String, String, String)>,
) {
    let mut check = |field: &str, zero_x: String, v1: String| {
        if zero_x != v1 {
            diffs.push((field.to_string(), zero_x, v1));
        }
    };

    check(
        "state",
        golden.state.clone(),
        state_name(view.state).to_string(),
    );
    check(
        "outcome",
        golden.outcome.clone().unwrap_or_else(|| "unknown".into()),
        outcome_name(view.outcome).to_string(),
    );
    if let (Some(won), Some(lost)) = (golden.games_won, golden.games_lost) {
        check(
            "score",
            format!("{won}-{lost}"),
            format!("{}-{}", view.wins, view.losses),
        );
    }
    check(
        "format",
        format!("{:?}", unknown_as_none(&golden.format)),
        format!("{:?}", view.format),
    );
    check(
        "match_type",
        format!("{:?}", unknown_as_none(&golden.match_type)),
        format!("{:?}", view.match_type),
    );
    check(
        "opponent",
        format!("{:?}", golden.opponent),
        format!("{:?}", view.opponent),
    );
    check(
        "started_at",
        format!("{:?}", golden.started_at),
        format!("{:?}", Some(view.started_at_ms)),
    );
    check(
        "ended_at",
        format!("{:?}", golden.ended_at),
        format!("{:?}", view.ended_at_ms),
    );

    for golden_game in &golden.games {
        let field = |name: &str| format!("games[{}].{name}", golden_game.mtgo_id);
        let Some(game) = view
            .games
            .iter()
            .find(|g| g.mtgo_game_id == Some(golden_game.mtgo_id))
        else {
            check(&field("present"), "true".into(), "false".into());
            continue;
        };
        let v1_won = match game.result {
            GameResult::Win => Some(true),
            GameResult::Loss => Some(false),
            GameResult::Unknown => None,
        };
        check(
            &field("won"),
            format!("{:?}", golden_game.won),
            format!("{v1_won:?}"),
        );
        if let Some(on_play) = game.on_play {
            check(
                &field("local_on_play"),
                format!("{:?}", golden_game.local_on_play),
                format!("{:?}", Some(on_play)),
            );
        }
        let counts = [
            (
                "local_mulligans",
                golden_game.local_mulligans,
                game.local_mulligans,
            ),
            (
                "opp_mulligans",
                golden_game.opp_mulligans,
                game.opp_mulligans,
            ),
            (
                "local_die_roll",
                golden_game.local_die_roll,
                game.local_die_roll,
            ),
            ("opp_die_roll", golden_game.opp_die_roll, game.opp_die_roll),
        ];
        for (name, zero_x, v1) in counts {
            // 0.x fills these only in its card stats job, which skips matches with
            // no linked deck version (`ComputeCardGameStats::handle`), so a null
            // there means "not computed", not "not in the log".
            if zero_x.is_some() {
                check(&field(name), format!("{zero_x:?}"), format!("{v1:?}"));
            }
        }
    }
}

/// The allowlist entries this test checks: those of a log fixture. The
/// `match_report`, `sync_*`, `card_stats*`, `overlay`, `replays` and
/// `leagues` entries are other crates' tests, and `archetypes` holds the informational archetype
/// deviations (archetypes spec section 19); any other fixture name is a
/// mistake and fails.
fn log_fixture_entries(
    allowlist: &[Allowed],
    fixture_names: &BTreeSet<String>,
) -> BTreeSet<DiffKey> {
    let mut kept = BTreeSet::new();
    for a in allowlist {
        if a.fixture == "match_report"
            || a.fixture.starts_with("sync_")
            || a.fixture.starts_with("card_stats")
            || a.fixture == "overlay"
            || a.fixture == "replays"
            || a.fixture == "leagues"
            || a.fixture == "archetypes"
        {
            continue;
        }
        assert!(
            fixture_names.contains(&a.fixture),
            "allowlist entry names an unknown fixture: {}",
            a.fixture
        );
        kept.insert((a.fixture.clone(), a.token.clone(), a.field.clone()));
    }
    kept
}

fn entry(fixture: &str) -> Allowed {
    Allowed {
        fixture: fixture.to_string(),
        token: "q-token".to_string(),
        field: "q-field".to_string(),
        reason: "q-reason".to_string(),
    }
}

// The match report, sync bundle, card stats, overlay and leagues entries belong to
// other crates' tests; a log fixture entry is kept.
#[test]
fn only_match_report_and_sync_entries_are_left_to_other_tests() {
    let names: BTreeSet<String> = ["q_log".to_string()].into_iter().collect();
    let allowlist = vec![
        entry("q_log"),
        entry("match_report"),
        entry("sync_match_bundles"),
        entry("card_stats_draft_hobbit"),
        entry("card_stats"),
        entry("overlay"),
        entry("leagues"),
        entry("archetypes"),
    ];
    let kept = log_fixture_entries(&allowlist, &names);
    assert_eq!(
        kept.into_iter().map(|key| key.0).collect::<Vec<_>>(),
        vec!["q_log".to_string()]
    );
}

// A misspelled fixture name is an error, never silently skipped.
#[test]
#[should_panic(expected = "unknown fixture")]
fn an_allowlist_entry_with_an_unknown_fixture_fails() {
    let names: BTreeSet<String> = ["q_log".to_string()].into_iter().collect();
    log_fixture_entries(&[entry("q_lgo")], &names);
}

#[test]
fn v1_matches_0x_or_differences_are_justified() {
    let parity_dir = fixtures_dir().join("parity");
    let allowlist: Vec<Allowed> = serde_json::from_str(
        &std::fs::read_to_string(parity_dir.join("allowlist.json")).expect("allowlist exists"),
    )
    .expect("allowlist is valid JSON");
    let fixture_names: BTreeSet<String> = fixture_tools::fixture_names().into_iter().collect();
    let allowed = log_fixture_entries(&allowlist, &fixture_names);

    // Every difference found, as (key used by the allowlist, human-readable line).
    let mut differences: Vec<(DiffKey, String)> = Vec::new();

    let mut golden_paths: Vec<_> = std::fs::read_dir(&parity_dir)
        .expect("parity dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.to_string_lossy().ends_with(".golden.json"))
        // The sync goldens (`sync_*`) are `sync-bundles`' tests, the card
        // stats goldens (`card_stats_*`) the tracker's.
        .filter(|p| {
            !p.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with("sync_") || name.starts_with("card_stats_")
            })
        })
        .collect();
    golden_paths.sort();

    for path in golden_paths {
        let golden: Golden = serde_json::from_str(&std::fs::read_to_string(&path).expect("golden"))
            .expect("golden JSON");
        let fixture = load_fixture(&golden.fixture);
        let mut projector = Projector::new(vec![]);
        for event in prepared_events(&fixture) {
            projector.apply(&event);
        }
        projector.finalize(fixture.meta.now_ms());

        let mut compared = BTreeSet::new();
        for golden_match in golden.matches.iter().filter(|m| m.state != "started") {
            compared.insert(golden_match.token.clone());
            let mut diffs = Vec::new();
            match projector.view(&golden_match.token) {
                Some(view) => compare_match(golden_match, view, &mut diffs),
                None => diffs.push(("match".into(), "present".into(), "missing".into())),
            }
            for (field, zero_x, v1) in diffs {
                let key = (
                    golden.fixture.clone(),
                    golden_match.token.clone(),
                    field.clone(),
                );
                let line = format!(
                    "{} {} {field}: 0.x={zero_x} v1={v1}",
                    golden.fixture, golden_match.token
                );
                differences.push((key, line));
            }
        }
        for view in projector.views() {
            if !compared.contains(&view.token) {
                let key = (
                    golden.fixture.clone(),
                    view.token.clone(),
                    "extra_match".to_string(),
                );
                let line = format!(
                    "{} {} extra_match: v1 shows a match 0.x does not",
                    golden.fixture, view.token
                );
                differences.push((key, line));
            }
        }
    }

    let unexplained: Vec<&str> = differences
        .iter()
        .filter(|(key, _)| !allowed.contains(key))
        .map(|(_, line)| line.as_str())
        .collect();
    let found: BTreeSet<DiffKey> = differences.iter().map(|(key, _)| key.clone()).collect();
    let stale: Vec<&DiffKey> = allowed.iter().filter(|key| !found.contains(*key)).collect();

    assert!(
        unexplained.is_empty(),
        "unexplained parity differences:\n{}",
        unexplained.join("\n")
    );
    assert!(
        stale.is_empty(),
        "allowlist entries that no longer occur: {stale:?}"
    );
}
