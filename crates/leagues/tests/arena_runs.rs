//! Arena limited runs (Arena limited runs spec section 3): a run per course,
//! matches joining the course of their event, orphans, completion by module
//! and by disappearance, decks per match, and live equals rebuild.

use leagues::{
    ArenaRun, ArenaRunMatch, CourseRecord, CourseSnapshot, CourseState, LeagueKind,
    PulledCourseRun, PulledRunMatch, RunState, arena_course_id, arena_run_name, fold_arena_runs,
    fold_arena_runs_merged, is_arena_run_key, join_records, newer_record, same_pool,
};

const DIRECT: &str = "ArenaDirect_EOE_Play_Sealed_20261106";
const DIRECT_FORMAT: &str = "S6EOE";
const DRAFT: &str = "PremierDraft_FRA_20260929";
const DRAFT_FORMAT: &str = "DFRAFRAFRA";
const COURSE_A: &str = "aaaaaaaa-0000-4000-8000-00000000000a";
const COURSE_B: &str = "bbbbbbbb-0000-4000-8000-00000000000b";
const COURSE_C: &str = "cccccccc-0000-4000-8000-00000000000c";
const FILE: i64 = 1;
const MINUTE: i64 = 60_000;

fn course(course_id: &str, event_name: &str, module: &str) -> CourseState {
    CourseState {
        course_id: course_id.to_string(),
        event_name: event_name.to_string(),
        module: module.to_string(),
        wins: 0,
        losses: 0,
        main: vec![(100, 4), (101, 2)],
        side: vec![(102, 1)],
        pool: vec![100, 101, 102],
    }
}

fn snapshot(offset: i64, observed_at_ms: i64, courses: Vec<CourseState>) -> CourseSnapshot {
    CourseSnapshot {
        file_id: FILE,
        offset,
        observed_at_ms,
        courses,
    }
}

fn a_match(token: &str, event_name: &str, format_code: &str, offset: i64) -> ArenaRunMatch {
    ArenaRunMatch {
        token: token.to_string(),
        event_name: event_name.to_string(),
        format_code: format_code.to_string(),
        file_id: FILE,
        offset,
        started_at_ms: offset * MINUTE,
        ended_at_ms: Some(offset * MINUTE + 10 * MINUTE),
        cards: Vec::new(),
    }
}

/// A sealed pool as catalog ids: `first` and the 39 ids after it.
fn pool(first: u32) -> Vec<u32> {
    (first..first + 40).collect()
}

/// A match whose game 1 deck and sideboard held `cards`.
fn played_with(mut played: ArenaRunMatch, cards: Vec<u32>) -> ArenaRunMatch {
    played.cards = cards;
    played
}

fn run<'a>(runs: &'a [ArenaRun], run_key: &str) -> &'a ArenaRun {
    runs.iter()
        .find(|run| run.run_key == run_key)
        .unwrap_or_else(|| panic!("no run {run_key}"))
}

#[test]
fn a_course_is_one_run_with_its_matches() {
    let snapshots = vec![snapshot(
        10,
        10 * MINUTE,
        vec![course(COURSE_A, DIRECT, "CreateMatch")],
    )];
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 20),
        a_match("m2", DIRECT, DIRECT_FORMAT, 30),
    ];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.run_key, format!("arena:{COURSE_A}"));
    assert_eq!(run.course_id.as_deref(), Some(COURSE_A));
    assert_eq!(run.event_name, DIRECT);
    assert_eq!(run.name, "Arena Direct · EOE Sealed");
    assert_eq!(run.kind, LeagueKind::Sealed);
    assert_eq!(run.format_code, DIRECT_FORMAT);
    assert_eq!(run.set_code.as_deref(), Some("EOE"));
    assert_eq!(run.state, RunState::Active);
    assert_eq!(run.started_at_ms, 20 * MINUTE);
    assert_eq!(run.completed_at_ms, None);
    assert_eq!(run.match_tokens, vec!["m1".to_string(), "m2".to_string()]);
    assert_eq!(run.decks.len(), 2);
}

#[test]
fn a_course_without_matches_starts_at_its_first_observation() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DRAFT, "CreateMatch")],
        ),
        snapshot(
            20,
            20 * MINUTE,
            vec![course(COURSE_A, DRAFT, "CreateMatch")],
        ),
    ];

    let runs = fold_arena_runs(&snapshots, &[]);

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].started_at_ms, 10 * MINUTE);
    assert_eq!(runs[0].kind, LeagueKind::Draft);
    assert_eq!(runs[0].format_code, DRAFT_FORMAT);
    assert_eq!(runs[0].set_code.as_deref(), Some("FRA"));
    assert!(runs[0].match_tokens.is_empty());
}

#[test]
fn matches_join_their_own_event() {
    let snapshots = vec![snapshot(
        10,
        10 * MINUTE,
        vec![
            course(COURSE_A, DIRECT, "CreateMatch"),
            course(COURSE_B, DRAFT, "CreateMatch"),
        ],
    )];
    let matches = vec![
        a_match("d1", DRAFT, DRAFT_FORMAT, 20),
        a_match("s1", DIRECT, DIRECT_FORMAT, 30),
        a_match("d2", DRAFT, DRAFT_FORMAT, 40),
    ];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 2);
    let direct = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(direct.match_tokens, vec!["s1".to_string()]);
    let draft = run(&runs, &format!("arena:{COURSE_B}"));
    assert_eq!(draft.match_tokens, vec!["d1".to_string(), "d2".to_string()]);
    // Runs are ordered by start: the draft's first match came first.
    assert_eq!(runs[0].run_key, draft.run_key);
}

#[test]
fn a_match_before_its_course_joins_it() {
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 10),
        a_match("m2", DIRECT, DIRECT_FORMAT, 20),
    ];
    let snapshots = vec![snapshot(
        30,
        30 * MINUTE,
        vec![course(COURSE_A, DIRECT, "CreateMatch")],
    )];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.run_key, format!("arena:{COURSE_A}"));
    assert_eq!(run.match_tokens, vec!["m1".to_string(), "m2".to_string()]);
    assert_eq!(run.started_at_ms, 10 * MINUTE);
    // No observation preceded these matches, so they carry no deck.
    assert!(run.decks.is_empty());
}

#[test]
fn an_orphan_match_without_a_course_is_its_own_run() {
    let matches = vec![
        a_match("m1", DRAFT, DRAFT_FORMAT, 10),
        a_match("m2", DRAFT, DRAFT_FORMAT, 20),
    ];
    // A later snapshot lists another event only: the draft never shows.
    let snapshots = vec![snapshot(
        30,
        30 * MINUTE,
        vec![course(COURSE_A, DIRECT, "CreateMatch")],
    )];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 2);
    let orphan = run(&runs, &format!("arena:{DRAFT}:m1"));
    assert_eq!(orphan.course_id, None);
    assert_eq!(orphan.event_name, DRAFT);
    assert_eq!(orphan.name, "Premier Draft · FRA");
    assert_eq!(orphan.kind, LeagueKind::Draft);
    assert_eq!(orphan.format_code, DRAFT_FORMAT);
    assert_eq!(orphan.set_code.as_deref(), Some("FRA"));
    assert_eq!(orphan.state, RunState::Active);
    assert_eq!(orphan.started_at_ms, 10 * MINUTE);
    assert_eq!(
        orphan.match_tokens,
        vec!["m1".to_string(), "m2".to_string()]
    );
    assert!(orphan.decks.is_empty());
}

#[test]
fn a_complete_module_completes_the_run() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(30, 30 * MINUTE, vec![course(COURSE_A, DIRECT, "Complete")]),
        snapshot(40, 40 * MINUTE, vec![course(COURSE_A, DIRECT, "Complete")]),
    ];
    let matches = vec![a_match("m1", DIRECT, DIRECT_FORMAT, 20)];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Complete);
    // The observation that completed it, not the later one repeating it; the
    // match ended at 30 minutes, no later.
    assert_eq!(runs[0].completed_at_ms, Some(30 * MINUTE));

    let claim = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(
            30,
            30 * MINUTE,
            vec![course(COURSE_A, DIRECT, "ClaimPrize")],
        ),
    ];
    let runs = fold_arena_runs(&claim, &matches);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at_ms, Some(30 * MINUTE));
}

#[test]
fn a_match_ending_after_the_completing_observation_sets_the_completion_time() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(21, 21 * MINUTE, vec![course(COURSE_A, DIRECT, "Complete")]),
    ];
    // The match started at 20 minutes and ended at 30: after the observation.
    let matches = vec![a_match("m1", DIRECT, DIRECT_FORMAT, 20)];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at_ms, Some(30 * MINUTE));
}

#[test]
fn a_course_missing_from_a_later_snapshot_completes_the_run() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![
                course(COURSE_A, DIRECT, "CreateMatch"),
                course(COURSE_B, DRAFT, "CreateMatch"),
            ],
        ),
        snapshot(
            30,
            30 * MINUTE,
            vec![course(COURSE_B, DRAFT, "CreateMatch")],
        ),
        snapshot(40, 40 * MINUTE, Vec::new()),
    ];
    let matches = vec![a_match("m1", DIRECT, DIRECT_FORMAT, 20)];

    let runs = fold_arena_runs(&snapshots, &matches);

    let direct = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(direct.state, RunState::Complete);
    assert_eq!(direct.completed_at_ms, Some(30 * MINUTE));
    // The empty snapshot completes the draft too.
    let draft = run(&runs, &format!("arena:{COURSE_B}"));
    assert_eq!(draft.state, RunState::Complete);
    assert_eq!(draft.completed_at_ms, Some(40 * MINUTE));
}

#[test]
fn a_match_after_its_course_completed_waits_for_the_next_course() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(30, 30 * MINUTE, Vec::new()),
        snapshot(
            50,
            50 * MINUTE,
            vec![course(COURSE_C, DIRECT, "CreateMatch")],
        ),
    ];
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 20),
        a_match("m2", DIRECT, DIRECT_FORMAT, 40),
        a_match("m3", DIRECT, DIRECT_FORMAT, 60),
    ];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 2);
    let first = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(first.match_tokens, vec!["m1".to_string()]);
    let second = run(&runs, &format!("arena:{COURSE_C}"));
    assert_eq!(
        second.match_tokens,
        vec!["m2".to_string(), "m3".to_string()]
    );
    assert_eq!(second.started_at_ms, 40 * MINUTE);
}

#[test]
fn a_course_listed_again_is_active_again() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(20, 20 * MINUTE, Vec::new()),
        snapshot(
            30,
            30 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
    ];
    let matches = vec![a_match("m1", DIRECT, DIRECT_FORMAT, 40)];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs[0].completed_at_ms, None);
    assert_eq!(runs[0].match_tokens, vec!["m1".to_string()]);
}

#[test]
fn each_match_takes_the_deck_observed_before_it() {
    let mut first = course(COURSE_A, DIRECT, "CreateMatch");
    first.main = vec![(100, 4)];
    first.side = vec![(101, 1)];
    let mut second = course(COURSE_A, DIRECT, "CreateMatch");
    second.main = vec![(100, 3), (101, 1)];
    second.side = Vec::new();
    let snapshots = vec![
        snapshot(10, 10 * MINUTE, vec![first]),
        snapshot(30, 30 * MINUTE, vec![second]),
    ];
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 20),
        a_match("m2", DIRECT, DIRECT_FORMAT, 40),
    ];

    let runs = fold_arena_runs(&snapshots, &matches);

    let decks = &runs[0].decks;
    assert_eq!(decks.len(), 2);
    assert_eq!(decks[0].token, "m1");
    assert_eq!((decks[0].file_id, decks[0].offset), (FILE, 10));
    assert_eq!(decks[0].main, vec![(100, 4)]);
    assert_eq!(decks[0].side, vec![(101, 1)]);
    assert_eq!(decks[1].token, "m2");
    assert_eq!((decks[1].file_id, decks[1].offset), (FILE, 30));
    assert_eq!(decks[1].main, vec![(100, 3), (101, 1)]);
    assert!(decks[1].side.is_empty());
}

/// A mixed sequence: a course with matches, an orphan that a later course
/// adopts, completion by module, disappearance and a return, and a match
/// before any snapshot.
fn mixed_sequence() -> (Vec<CourseSnapshot>, Vec<ArenaRunMatch>) {
    let snapshots = vec![
        snapshot(
            20,
            20 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(
            40,
            40 * MINUTE,
            vec![
                course(COURSE_A, DIRECT, "CreateMatch"),
                course(COURSE_B, DRAFT, "CreateMatch"),
            ],
        ),
        snapshot(60, 60 * MINUTE, vec![course(COURSE_B, DRAFT, "Complete")]),
        snapshot(80, 80 * MINUTE, Vec::new()),
        snapshot(
            100,
            100 * MINUTE,
            vec![course(COURSE_C, DIRECT, "CreateMatch")],
        ),
        snapshot(
            120,
            120 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
    ];
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 10),
        a_match("m2", DRAFT, DRAFT_FORMAT, 30),
        a_match("m3", DIRECT, DIRECT_FORMAT, 50),
        a_match("m4", DIRECT, DIRECT_FORMAT, 70),
        a_match("m5", DIRECT, DIRECT_FORMAT, 90),
        a_match("m6", DRAFT, DRAFT_FORMAT, 110),
        a_match("m7", DIRECT, DIRECT_FORMAT, 130),
    ];
    (snapshots, matches)
}

#[test]
fn folding_prefixes_equals_folding_all() {
    let (snapshots, matches) = mixed_sequence();
    let all = fold_arena_runs(&snapshots, &matches);

    // Every log place in the sequence, in order.
    let mut places: Vec<i64> = snapshots.iter().map(|snapshot| snapshot.offset).collect();
    places.extend(matches.iter().map(|m| m.offset));
    places.sort_unstable();

    let mut previous: Vec<ArenaRun> = Vec::new();
    for place in places {
        let prefix_snapshots: Vec<CourseSnapshot> = snapshots
            .iter()
            .filter(|snapshot| snapshot.offset <= place)
            .cloned()
            .collect();
        let prefix_matches: Vec<ArenaRunMatch> = matches
            .iter()
            .filter(|m| m.offset <= place)
            .cloned()
            .collect();

        // The fold is a function of its inputs: arrival order never matters.
        let folded = fold_arena_runs(&prefix_snapshots, &prefix_matches);
        let reversed_snapshots: Vec<CourseSnapshot> =
            prefix_snapshots.iter().rev().cloned().collect();
        let reversed_matches: Vec<ArenaRunMatch> = prefix_matches.iter().rev().cloned().collect();
        assert_eq!(
            fold_arena_runs(&reversed_snapshots, &reversed_matches),
            folded,
            "prefix to {place} depends on arrival order"
        );

        // Once a course is known, its key and matches never leave.
        for run in &previous {
            if run.course_id.is_none() {
                continue;
            }
            let now = folded
                .iter()
                .find(|candidate| candidate.run_key == run.run_key)
                .unwrap_or_else(|| panic!("run {} vanished at {place}", run.run_key));
            assert!(
                now.match_tokens.starts_with(&run.match_tokens),
                "run {} lost matches at {place}",
                run.run_key
            );
        }
        previous = folded;
    }
    assert_eq!(previous, all);

    // The whole sequence, as the rules say.
    let a = run(&all, &format!("arena:{COURSE_A}"));
    assert_eq!(
        a.match_tokens,
        vec!["m1".to_string(), "m3".to_string(), "m7".to_string()]
    );
    assert_eq!(a.state, RunState::Active);
    let b = run(&all, &format!("arena:{COURSE_B}"));
    assert_eq!(b.match_tokens, vec!["m2".to_string()]);
    assert_eq!(b.state, RunState::Complete);
    assert_eq!(b.completed_at_ms, Some(60 * MINUTE));
    let c = run(&all, &format!("arena:{COURSE_C}"));
    assert_eq!(c.match_tokens, vec!["m4".to_string(), "m5".to_string()]);
    assert_eq!(c.state, RunState::Complete);
    assert_eq!(c.completed_at_ms, Some(120 * MINUTE));
    let orphan = run(&all, &format!("arena:{DRAFT}:m6"));
    assert_eq!(orphan.match_tokens, vec!["m6".to_string()]);
    assert_eq!(all.len(), 4);
}

#[test]
fn run_names() {
    assert_eq!(
        arena_run_name("ArenaDirect_EOE_Play_Sealed_20261106"),
        "Arena Direct · EOE Sealed"
    );
    assert_eq!(
        arena_run_name("PremierDraft_FRA_20260929"),
        "Premier Draft · FRA"
    );
    assert_eq!(
        arena_run_name("QuickDraft_FDN_20261101"),
        "Quick Draft · FDN"
    );
    assert_eq!(arena_run_name("TradDraft_FRA_x"), "Traditional Draft · FRA");
    assert_eq!(arena_run_name("Sealed_FDN_x"), "Sealed · FDN");
    assert_eq!(
        arena_run_name("TradSealed_FRA_x"),
        "Traditional Sealed · FRA"
    );
    assert_eq!(arena_run_name("CubeDraft_FDN_x"), "FDN Draft");
    assert_eq!(arena_run_name("OpenSealed_FDN_x"), "FDN Sealed");
    assert_eq!(arena_run_name("Historic_Play"), "Historic_Play");
}

#[test]
fn an_arena_run_key_names_its_course() {
    assert!(is_arena_run_key("arena:q-course"));
    assert!(is_arena_run_key("arena:PremierDraft_FRA_20260929:q-m1"));
    assert!(!is_arena_run_key("league:q-m1"));
    assert_eq!(arena_course_id("arena:q-course"), Some("q-course"));
    assert_eq!(
        arena_course_id("arena:PremierDraft_FRA_20260929:q-m1"),
        None
    );
    assert_eq!(arena_course_id("league:q-m1"), None);
}

/// A snapshot listing the finished entry of an event and a fresh one: new
/// matches join the fresh course, whatever order the courses are listed in.
fn re_entry_runs(courses: Vec<CourseState>) -> Vec<ArenaRun> {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(30, 30 * MINUTE, courses),
    ];
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 20),
        a_match("m2", DIRECT, DIRECT_FORMAT, 40),
    ];
    fold_arena_runs(&snapshots, &matches)
}

#[test]
fn a_re_entry_listed_after_the_finished_course_takes_new_matches() {
    let runs = re_entry_runs(vec![
        course(COURSE_A, DIRECT, "Complete"),
        course(COURSE_C, DIRECT, "CreateMatch"),
    ]);

    let finished = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(finished.state, RunState::Complete);
    assert_eq!(finished.match_tokens, vec!["m1".to_string()]);
    let fresh = run(&runs, &format!("arena:{COURSE_C}"));
    assert_eq!(fresh.state, RunState::Active);
    assert_eq!(fresh.match_tokens, vec!["m2".to_string()]);
}

#[test]
fn a_re_entry_listed_before_the_finished_course_takes_new_matches() {
    let runs = re_entry_runs(vec![
        course(COURSE_C, DIRECT, "CreateMatch"),
        course(COURSE_A, DIRECT, "ClaimPrize"),
    ]);

    let finished = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(finished.state, RunState::Complete);
    assert_eq!(finished.match_tokens, vec!["m1".to_string()]);
    let fresh = run(&runs, &format!("arena:{COURSE_C}"));
    assert_eq!(fresh.match_tokens, vec!["m2".to_string()]);
}

#[test]
fn a_match_while_the_only_course_is_complete_waits_for_the_next_course() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![course(COURSE_A, DIRECT, "CreateMatch")],
        ),
        snapshot(30, 30 * MINUTE, vec![course(COURSE_A, DIRECT, "Complete")]),
        // Still listed as finished after the new entry's first match.
        snapshot(50, 50 * MINUTE, vec![course(COURSE_A, DIRECT, "Complete")]),
        snapshot(
            70,
            70 * MINUTE,
            vec![
                course(COURSE_A, DIRECT, "Complete"),
                course(COURSE_C, DIRECT, "CreateMatch"),
            ],
        ),
    ];
    let matches = vec![
        a_match("m1", DIRECT, DIRECT_FORMAT, 20),
        a_match("m2", DIRECT, DIRECT_FORMAT, 40),
        a_match("m3", DIRECT, DIRECT_FORMAT, 80),
    ];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 2);
    let finished = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(finished.match_tokens, vec!["m1".to_string()]);
    let fresh = run(&runs, &format!("arena:{COURSE_C}"));
    assert_eq!(fresh.match_tokens, vec!["m2".to_string(), "m3".to_string()]);
    assert_eq!(fresh.started_at_ms, 40 * MINUTE);

    // With no next course yet, the match is its own run, not the finished one.
    let runs = fold_arena_runs(&snapshots[..3], &matches[..2]);
    assert_eq!(runs.len(), 2);
    let finished = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(finished.match_tokens, vec!["m1".to_string()]);
    let orphan = run(&runs, &format!("arena:{DIRECT}:m2"));
    assert_eq!(orphan.match_tokens, vec!["m2".to_string()]);
}

#[test]
fn course_less_matches_of_one_event_split_where_the_pool_changes() {
    // No course ever shows (snapshots lost to an older log): two Direct
    // entries, each its own sealed pool, plus a match with no game data.
    let matches = vec![
        played_with(a_match("m1", DIRECT, DIRECT_FORMAT, 10), pool(100)),
        played_with(a_match("m2", DIRECT, DIRECT_FORMAT, 20), pool(100)),
        played_with(a_match("m3", DIRECT, DIRECT_FORMAT, 30), pool(500)),
        a_match("m4", DIRECT, DIRECT_FORMAT, 40),
        // The same pool with a basic land added still is the same entry.
        played_with(a_match("m5", DIRECT, DIRECT_FORMAT, 50), {
            let mut cards = pool(500);
            cards.push(9);
            cards
        }),
    ];

    let runs = fold_arena_runs(&[], &matches);

    assert_eq!(runs.len(), 2);
    let first = run(&runs, &format!("arena:{DIRECT}:m1"));
    assert_eq!(first.match_tokens, vec!["m1".to_string(), "m2".to_string()]);
    let second = run(&runs, &format!("arena:{DIRECT}:m3"));
    assert_eq!(
        second.match_tokens,
        vec!["m3".to_string(), "m4".to_string(), "m5".to_string()]
    );
    assert_eq!(second.started_at_ms, 30 * MINUTE);
}

#[test]
fn a_match_from_another_pool_never_joins_the_course() {
    let mut direct = course(COURSE_A, DIRECT, "CreateMatch");
    direct.pool = pool(100);
    // m1 waits (no course yet) but was played with another pool: an older
    // entry whose course never showed. m3 comes while the course is open but
    // is from another pool too (the snapshot that would end it was lost).
    let snapshots = vec![snapshot(20, 20 * MINUTE, vec![direct])];
    let matches = vec![
        played_with(a_match("m1", DIRECT, DIRECT_FORMAT, 10), pool(500)),
        played_with(a_match("m2", DIRECT, DIRECT_FORMAT, 30), pool(100)),
        played_with(a_match("m3", DIRECT, DIRECT_FORMAT, 40), pool(900)),
        a_match("m4", DIRECT, DIRECT_FORMAT, 50),
    ];

    let runs = fold_arena_runs(&snapshots, &matches);

    assert_eq!(runs.len(), 3);
    let course_run = run(&runs, &format!("arena:{COURSE_A}"));
    assert_eq!(
        course_run.match_tokens,
        vec!["m2".to_string(), "m4".to_string()]
    );
    let older = run(&runs, &format!("arena:{DIRECT}:m1"));
    assert_eq!(older.match_tokens, vec!["m1".to_string()]);
    let other = run(&runs, &format!("arena:{DIRECT}:m3"));
    assert_eq!(other.match_tokens, vec!["m3".to_string()]);
}

#[test]
fn same_pool_needs_nine_in_ten_of_the_shorter_list() {
    let pool: Vec<u32> = (100..110).collect();
    // Exactly nine of ten shared: the same pool.
    let nine: Vec<u32> = (101..111).collect();
    assert_eq!(same_pool(&pool, &nine), Some(true));
    // Eight of ten: just under, another pool.
    let eight: Vec<u32> = (102..112).collect();
    assert_eq!(same_pool(&pool, &eight), Some(false));
    // Copies and order do not count.
    let copies = vec![109, 100, 100, 101, 102, 103, 104, 105, 106, 107, 108];
    assert_eq!(same_pool(&pool, &copies), Some(true));
}

#[test]
fn same_pool_measures_the_shorter_list() {
    let large: Vec<u32> = (100..160).collect();
    // A deck list inside a whole pool is that pool.
    let deck: Vec<u32> = (120..143).collect();
    assert_eq!(same_pool(&deck, &large), Some(true));
    assert_eq!(same_pool(&large, &deck), Some(true));
    // A deck list with a few cards from elsewhere is not.
    let mut mixed: Vec<u32> = (120..140).collect();
    mixed.extend([900, 901, 902]);
    assert_eq!(same_pool(&mixed, &large), Some(false));
}

#[test]
fn shared_basic_lands_alone_are_not_one_pool() {
    let basics = [1, 2, 3, 4, 5];
    let mut first: Vec<u32> = (100..140).collect();
    let mut second: Vec<u32> = (500..540).collect();
    first.extend(basics);
    second.extend(basics);
    assert_eq!(same_pool(&first, &second), Some(false));
}

#[test]
fn an_empty_list_is_unknown() {
    assert_eq!(same_pool(&[], &[1, 2]), None);
    assert_eq!(same_pool(&[1, 2], &[]), None);
}

// ---- Runs across devices (cross-device sync spec section 8) ----------------

fn record(wins: u32, losses: u32, state: RunState, observed_at_ms: i64) -> CourseRecord {
    CourseRecord {
        wins,
        losses,
        state,
        observed_at_ms,
    }
}

fn with_record(mut state: CourseState, wins: u32, losses: u32) -> CourseState {
    state.wins = wins;
    state.losses = losses;
    state
}

fn pulled(course_id: &str, event_name: &str, record: CourseRecord) -> PulledCourseRun {
    PulledCourseRun {
        course_id: course_id.to_string(),
        event_name: event_name.to_string(),
        record,
    }
}

#[test]
fn the_newest_local_snapshot_is_the_runs_record() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![with_record(course(COURSE_A, DRAFT, "CreateMatch"), 1, 0)],
        ),
        snapshot(
            20,
            20 * MINUTE,
            vec![with_record(course(COURSE_A, DRAFT, "CreateMatch"), 2, 0)],
        ),
    ];
    let runs = fold_arena_runs(&snapshots, &[]);
    assert_eq!(
        runs[0].record,
        Some(record(2, 0, RunState::Active, 20 * MINUTE))
    );
}

#[test]
fn a_course_that_leaves_the_snapshots_keeps_its_record_complete() {
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![with_record(course(COURSE_A, DRAFT, "CreateMatch"), 2, 1)],
        ),
        snapshot(20, 20 * MINUTE, Vec::new()),
        snapshot(30, 30 * MINUTE, Vec::new()),
    ];
    let runs = fold_arena_runs(&snapshots, &[]);
    assert_eq!(
        runs[0].record,
        Some(record(2, 1, RunState::Complete, 10 * MINUTE)),
        "its counts as last listed, at that listing's time"
    );
}

#[test]
fn pulled_snapshot_newer_than_local_wins() {
    let snapshots = vec![snapshot(
        10,
        10 * MINUTE,
        vec![with_record(course(COURSE_A, DRAFT, "CreateMatch"), 2, 0)],
    )];
    let matches = vec![a_match("m1", DRAFT, DRAFT_FORMAT, 20)];

    let newer = pulled(
        COURSE_A,
        DRAFT,
        record(5, 1, RunState::Complete, 90 * MINUTE),
    );
    let runs = fold_arena_runs_merged(&snapshots, &matches, &[newer], &[]);
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(
        run.record,
        Some(record(5, 1, RunState::Complete, 90 * MINUTE))
    );
    assert_eq!(run.state, RunState::Complete);
    assert_eq!(run.completed_at_ms, Some(90 * MINUTE));
    assert_eq!(run.match_tokens, vec!["m1".to_string()]);

    // An older pulled snapshot never replaces the newer local one.
    let older = pulled(COURSE_A, DRAFT, record(1, 0, RunState::Active, 5 * MINUTE));
    let runs = fold_arena_runs_merged(&snapshots, &matches, &[older], &[]);
    assert_eq!(
        runs[0].record,
        Some(record(2, 0, RunState::Active, 10 * MINUTE))
    );
    assert_eq!(runs[0].state, RunState::Active);
    assert_eq!(runs, fold_arena_runs(&snapshots, &matches));
}

#[test]
fn a_tie_on_observed_time_picks_the_same_snapshot_on_every_device() {
    let one = record(2, 0, RunState::Active, 10 * MINUTE);
    let other = record(2, 1, RunState::Active, 10 * MINUTE);
    assert!(newer_record(&other, &one) != newer_record(&one, &other));
    assert!(!newer_record(&one, &one));
}

#[test]
fn course_seen_only_by_pull_yields_a_run() {
    let only = pulled(COURSE_B, DRAFT, record(3, 0, RunState::Active, 40 * MINUTE));
    let joined = PulledRunMatch {
        token: "m-phone".to_string(),
        course_id: COURSE_B.to_string(),
        started_at_ms: 30 * MINUTE,
        ended_at_ms: Some(35 * MINUTE),
    };
    let elsewhere = PulledRunMatch {
        token: "m-nowhere".to_string(),
        course_id: COURSE_C.to_string(),
        started_at_ms: 31 * MINUTE,
        ended_at_ms: None,
    };
    let runs = fold_arena_runs_merged(&[], &[], &[only], &[joined, elsewhere]);
    assert_eq!(runs.len(), 1, "a match of an unknown course makes no run");
    let run = &runs[0];
    assert_eq!(run.run_key, format!("arena:{COURSE_B}"));
    assert_eq!(arena_course_id(&run.run_key), Some(COURSE_B));
    assert_eq!(run.event_name, DRAFT);
    assert_eq!(run.kind, LeagueKind::Draft);
    assert_eq!(run.format_code, DRAFT_FORMAT);
    assert_eq!(run.state, RunState::Active);
    assert_eq!(run.started_at_ms, 30 * MINUTE);
    assert_eq!(run.match_tokens, vec!["m-phone".to_string()]);
    assert!(run.decks.is_empty());
    assert_eq!(
        run.record,
        Some(record(3, 0, RunState::Active, 40 * MINUTE))
    );
}

#[test]
fn pulled_matches_join_a_local_course_after_its_own() {
    let snapshots = vec![snapshot(
        10,
        10 * MINUTE,
        vec![course(COURSE_A, DRAFT, "CreateMatch")],
    )];
    let matches = vec![a_match("m1", DRAFT, DRAFT_FORMAT, 20)];
    let early = PulledRunMatch {
        token: "m0".to_string(),
        course_id: COURSE_A.to_string(),
        started_at_ms: 15 * MINUTE,
        ended_at_ms: Some(18 * MINUTE),
    };
    let runs = fold_arena_runs_merged(&snapshots, &matches, &[], &[early]);
    assert_eq!(runs.len(), 1);
    assert_eq!(
        runs[0].match_tokens,
        vec!["m1".to_string(), "m0".to_string()]
    );
    assert_eq!(runs[0].started_at_ms, 15 * MINUTE, "the earliest match");
    assert_eq!(runs[0].decks.len(), 1, "a pulled match brings no deck");
}

#[test]
fn a_course_gone_from_the_set_never_beats_an_earlier_pulled_record() {
    // A sees 2-0 at t1; B pushed 7-2 complete at t4; A sees the course gone
    // at t5. The absence says nothing about the counts.
    let snapshots = vec![
        snapshot(
            10,
            10 * MINUTE,
            vec![with_record(course(COURSE_A, DRAFT, "CreateMatch"), 2, 0)],
        ),
        snapshot(50, 50 * MINUTE, Vec::new()),
    ];
    let other = pulled(
        COURSE_A,
        DRAFT,
        record(7, 2, RunState::Complete, 40 * MINUTE),
    );
    let runs = fold_arena_runs_merged(&snapshots, &[], &[other], &[]);
    assert_eq!(
        runs[0].record,
        Some(record(7, 2, RunState::Complete, 40 * MINUTE))
    );
    assert_eq!(runs[0].state, RunState::Complete);
}

#[test]
fn either_side_complete_completes_the_merged_record() {
    let local = record(2, 1, RunState::Complete, 10 * MINUTE);
    let pulled = record(3, 1, RunState::Active, 20 * MINUTE);
    assert_eq!(
        join_records(&local, &pulled),
        record(3, 1, RunState::Complete, 20 * MINUTE)
    );
}

#[test]
fn joining_records_is_order_free() {
    let all = [
        record(2, 0, RunState::Active, 10 * MINUTE),
        record(2, 1, RunState::Active, 10 * MINUTE),
        record(1, 0, RunState::Complete, 5 * MINUTE),
        record(5, 1, RunState::Active, 30 * MINUTE),
    ];
    for a in &all {
        assert_eq!(join_records(a, a), a.clone(), "idempotent");
        for b in &all {
            assert_eq!(join_records(a, b), join_records(b, a), "commutative");
            for c in &all {
                assert_eq!(
                    join_records(&join_records(a, b), c),
                    join_records(a, &join_records(b, c)),
                    "associative"
                );
            }
        }
    }
}
