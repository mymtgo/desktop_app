//! Limited league runs in the fold (limited spec section 5). Ports 0.x
//! `ResolveDraftLeagueTest`, `LinkUnlinkedDraftsTest`,
//! `AdoptCourselessDraftLeagueTest`, `SealedLeagueTest` and the limited cases
//! of `AssignLeagueTest`, plus the limited classification and order
//! independence.
//!
//! 0.x ran these rules as separate actions over stored rows. Here every case
//! is one fold of inputs in log order: a draft is an input at its first log
//! line, a match at its start.

use chrono::NaiveDateTime;
use chrono_tz::Tz;
use leagues::{
    Classification, DraftInput, DraftTournament, ImportedRunSeed, LeagueFold, LeagueInput,
    LeagueKind, LeagueMatch, LimitedFacts, LimitedSeed, Mark, OrderedInput, PanelFormat, Run,
    RunInput, RunMatch, RunState, TOURNAMENT_DRAFT_WINDOW_MS, fold_league_runs, fold_runs,
    is_limited_format_code, is_tournament_run_key, summarize_limited_run, tournament_draft_label,
    tournament_draft_state, tournament_run_key,
};
use limited::{CardFacts, Draft, DraftState, Pick, RegisteredCard, RegisteredDeck};
use mtgo_match::{MatchState, Outcome};

const TOKEN: &str = "48a2e914-f2ee-4fce-a4ad-47e396488889";
const TOKEN_2: &str = "7f1e2d3c-4b5a-4968-8776-655443322110";
const DRAFT_TOKEN: &str = "791bacca-caea-4d88-b6c7-3bc067d412c2";
const DRAFT_TOKEN_2: &str = "aaaaaaaa-0000-4000-8000-000000000002";
const EVENT: i64 = 11039;
const COURSE: u64 = 35_746_768;
const UTC: Tz = chrono_tz::UTC;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60 * SECOND;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// UTC milliseconds of `YYYY-MM-DDTHH:MM:SS`.
fn utc(text: &str) -> i64 {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S")
        .unwrap_or_else(|error| panic!("{text}: {error}"))
        .and_utc()
        .timestamp_millis()
}

fn base() -> i64 {
    utc("2026-08-22T12:00:00")
}

/// A complete, won league match on `TOKEN`, 20 minutes long, in `format`.
fn match_in(format: Option<&str>, token: &str, started_at: i64) -> LeagueMatch {
    LeagueMatch {
        token: token.to_string(),
        league_token: TOKEN.to_string(),
        started_at,
        ended_at: Some(started_at + 20 * MINUTE),
        state: MatchState::Complete,
        outcome: Outcome::Win,
        format: format.map(str::to_string),
        game_structure: Some("HOBx3".to_string()),
        deck_version_id: None,
        games: Vec::new(),
    }
}

/// A draft league match (`DHOBHOBHOB`).
fn draft_match(token: &str, started_at: i64) -> LeagueMatch {
    match_in(Some("DHOBHOBHOB"), token, started_at)
}

/// A sealed league match (`S6FRA`).
fn sealed_match(token: &str, started_at: i64) -> LeagueMatch {
    let mut played = match_in(Some("S6FRA"), token, started_at);
    played.game_structure = Some("FRAx6".to_string());
    played
}

fn on_token(mut played: LeagueMatch, league_token: &str) -> LeagueMatch {
    played.league_token = league_token.to_string();
    played
}

fn with_version(mut played: LeagueMatch, deck_version_id: i64) -> LeagueMatch {
    played.deck_version_id = Some(deck_version_id);
    played
}

/// A match that started but never completed.
fn unfinished(mut played: LeagueMatch) -> LeagueMatch {
    played.state = MatchState::InProgress;
    played.ended_at = None;
    played
}

/// A picked card at `ordinal`.
fn pick(ordinal: u32, catalog_id: u32) -> Pick {
    Pick {
        ordinal,
        pack_number: (ordinal - 1) / 14 + 1,
        pick_number: (ordinal - 1) % 14 + 1,
        pack_id: None,
        direction: None,
        cards_available: vec![catalog_id],
        picked_catalog_id: Some(catalog_id),
        picked_card_id: Some(u64::from(catalog_id) + 1_000_000),
        reservations: Vec::new(),
        shown_at_ms: None,
        deadline_at_ms: None,
        picked_at_ms: None,
    }
}

/// A finished draft that picked each of `pool` once.
fn draft(
    token: &str,
    event_id: Option<i64>,
    course_id: Option<u64>,
    pool: impl IntoIterator<Item = u32>,
    started_at: i64,
) -> Draft {
    let picks: Vec<Pick> = pool
        .into_iter()
        .enumerate()
        .map(|(index, catalog_id)| pick(index as u32 + 1, catalog_id))
        .collect();
    Draft {
        token: token.to_string(),
        league_event_id: event_id.map(|id| id as u64),
        course_id,
        draft_id: Some(6_781_001),
        pod_token: None,
        seat_index: None,
        seat_count: 8,
        booster_catalog_id: None,
        state: DraftState::Finished,
        pack_size: 14,
        picks_expected: 42,
        started_at_ms: Some(started_at),
        ended_at_ms: Some(started_at + 25 * MINUTE),
        picks,
    }
}

fn picking(mut unfinished: Draft) -> Draft {
    unfinished.state = DraftState::Picking;
    unfinished.ended_at_ms = None;
    unfinished
}

/// One main deck copy of each card.
fn main_deck(ids: impl IntoIterator<Item = u32>) -> Vec<RegisteredCard> {
    ids.into_iter()
        .map(|catalog_id| RegisteredCard {
            catalog_id,
            quantity: 1,
            sideboard: false,
        })
        .collect()
}

/// Inputs in log order (one file, rising offsets) plus the limited facts.
#[derive(Default, Clone)]
struct Log {
    items: Vec<OrderedInput>,
    facts: LimitedFacts,
}

impl Log {
    fn next_offset(&self) -> u64 {
        (self.items.len() + self.facts.drafts.len()) as u64 * 100
    }

    fn push(&mut self, order_ms: i64, input: LeagueInput) {
        let offset = self.next_offset();
        self.items.push((order_ms, 1, offset, input));
    }

    fn game(&mut self, played: LeagueMatch) {
        self.push(played.started_at, LeagueInput::Match(played));
    }

    /// A panel view for `token` and `event_id`, showing `HOBx3`.
    fn joined(&mut self, token: &str, event_id: i64, at_ms: i64) {
        self.push(
            at_ms,
            LeagueInput::Joined {
                token: token.to_string(),
                event_id,
                at_ms,
                rounds: None,
            },
        );
        self.facts.panels.push(PanelFormat {
            event_id,
            at_ms,
            play_format: "HOBx3".to_string(),
        });
    }

    /// The player entering `token`'s league at `at_ms`: the join request,
    /// then the panel MTGO logs with it, stating `rounds` matches.
    fn entered(&mut self, token: &str, at_ms: i64, rounds: Option<usize>) {
        self.push(at_ms, LeagueInput::JoinRequested { at_ms });
        self.push(
            at_ms,
            LeagueInput::Joined {
                token: token.to_string(),
                event_id: EVENT,
                at_ms,
                rounds,
            },
        );
    }

    fn manual_drop(&mut self, run_key: &str, at_ms: i64) {
        self.items.push((
            at_ms,
            0,
            0,
            LeagueInput::ManualDrop {
                run_key: run_key.to_string(),
                at_ms,
            },
        ));
    }

    /// A draft at its first log line, `at_ms`.
    fn draft_at(&mut self, draft: Draft, at_ms: i64) {
        let offset = self.next_offset();
        self.facts.drafts.push(DraftInput {
            order_ms: at_ms,
            file_id: 1,
            offset,
            draft,
            run_key: None,
        });
    }

    /// A draft at its start.
    fn draft(&mut self, draft: Draft) {
        let at = draft.started_at_ms.expect("a start");
        self.draft_at(draft, at);
    }

    /// A 0.x draft 0.x linked to the imported run `run_key`.
    fn imported_draft(&mut self, draft: Draft, run_key: &str) {
        let offset = self.next_offset();
        self.facts.drafts.push(DraftInput {
            order_ms: draft.started_at_ms.unwrap_or(0),
            file_id: 0,
            offset,
            draft,
            run_key: Some(run_key.to_string()),
        });
    }

    fn imported(&mut self, seed: ImportedRunSeed, limited: LimitedSeed) {
        self.facts.seeds.insert(seed.key.clone(), limited);
        self.push(seed.started_at, LeagueInput::ImportedRun(seed));
    }

    /// The deck MTGO registered for match `token`.
    fn registered(&mut self, token: &str, cards: Vec<RegisteredCard>, at_ms: i64) {
        self.facts.registered.insert(
            token.to_string(),
            RegisteredDeck {
                match_token: Some(token.to_string()),
                signature: format!("sig-{token}"),
                captured_at_ms: at_ms,
                cards,
            },
        );
    }

    /// Cards with card data: creatures of `set_code`.
    fn known(&mut self, ids: impl IntoIterator<Item = u32>, set_code: Option<&str>) {
        for id in ids {
            self.facts.cards.insert(
                id,
                CardFacts {
                    name: Some(format!("Q card {id}")),
                    card_type: Some("Creature".to_string()),
                    set_code: set_code.map(str::to_string),
                    ..CardFacts::default()
                },
            );
        }
    }

    /// Cards with a row but no card data yet (0.x stubs).
    fn stubs(&mut self, ids: impl IntoIterator<Item = u32>) {
        for id in ids {
            self.facts.cards.insert(id, CardFacts::default());
        }
    }

    fn fold(&self) -> LeagueFold {
        fold_league_runs(self.items.clone(), self.facts.clone(), UTC)
    }
}

fn tokens(run: &Run) -> Vec<&str> {
    run.match_tokens.iter().map(String::as_str).collect()
}

fn run_of<'a>(fold: &'a LeagueFold, match_token: &str) -> &'a Run {
    fold.runs
        .iter()
        .find(|run| run.match_tokens.iter().any(|token| token == match_token))
        .unwrap_or_else(|| panic!("no run holds {match_token}"))
}

fn run_with_draft<'a>(fold: &'a LeagueFold, draft_token: &str) -> &'a Run {
    fold.runs
        .iter()
        .find(|run| run.draft_token.as_deref() == Some(draft_token))
        .unwrap_or_else(|| panic!("no run holds draft {draft_token}"))
}

fn of_event(fold: &LeagueFold, event_id: i64) -> Vec<&Run> {
    fold.runs
        .iter()
        .filter(|run| run.event_id == Some(event_id))
        .collect()
}

// 0.x SealedLeagueTest and LeagueKind

#[test]
fn format_codes_name_the_league_kind_and_its_rounds() {
    for (format, limited) in [
        ("S6FRA", true),
        ("S12FRA", true),
        ("DHOBHOBHOB", true),
        ("CMODERN", false),
        ("Standard", false),
        ("S", false),
    ] {
        assert_eq!(is_limited_format_code(format), limited, "{format}");
    }
    assert_eq!(
        LeagueKind::from_format_code(Some("S6FRA")),
        LeagueKind::Sealed
    );
    assert_eq!(
        LeagueKind::from_format_code(Some("DHOB")),
        LeagueKind::Draft
    );
    assert_eq!(
        LeagueKind::from_format_code(Some("CMODERN")),
        LeagueKind::Constructed
    );
    assert_eq!(LeagueKind::from_format_code(None), LeagueKind::Constructed);
    assert_eq!(LeagueKind::Sealed.round_count(), 5);
    assert_eq!(LeagueKind::Draft.round_count(), 3);
    assert_eq!(LeagueKind::Constructed.round_count(), 5);
    assert_eq!(
        [
            LeagueKind::Constructed.as_str(),
            LeagueKind::Draft.as_str(),
            LeagueKind::Sealed.as_str()
        ],
        ["constructed", "draft", "sealed"]
    );
    assert!(LeagueKind::Sealed.is_limited() && !LeagueKind::Constructed.is_limited());
}

#[test]
fn a_sealed_match_mints_a_sealed_run_with_its_set_code() {
    let mut log = Log::default();
    log.game(sealed_match("s1", base()));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.kind, LeagueKind::Sealed);
    assert_eq!(run.set_code.as_deref(), Some("FRA"));
    assert_eq!(run.key, format!("{TOKEN}:s1"));
    assert_eq!(run.name, "FRAx6 League 22-08-2026 12:00pm");
    assert_eq!(run.format.as_deref(), Some("S6FRA"));
    assert_eq!(run.state, RunState::Active);
}

#[test]
fn the_same_sealed_match_twice_makes_one_run() {
    let mut log = Log::default();
    log.game(sealed_match("s1", base()));
    log.game(sealed_match("s1", base()));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(fold.runs[0].kind, LeagueKind::Sealed);
    assert_eq!(tokens(&fold.runs[0]), vec!["s1"]);
}

#[test]
fn a_sealed_run_takes_a_fifth_match_but_not_a_sixth() {
    let mut log = Log::default();
    for index in 0..6 {
        log.game(unfinished(sealed_match(
            &format!("s{index}"),
            base() + index * HOUR,
        )));
    }
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 2);
    assert_eq!(tokens(&fold.runs[0]), vec!["s0", "s1", "s2", "s3", "s4"]);
    assert_eq!(fold.runs[0].state, RunState::Complete);
    assert_eq!(tokens(&fold.runs[1]), vec!["s5"]);
}

#[test]
fn entering_a_league_again_closes_its_run_whatever_its_length() {
    let mut log = Log::default();
    log.entered(TOKEN, base() - MINUTE, None);
    for index in 0..3 {
        log.game(sealed_match(&format!("a{index}"), base() + index * HOUR));
    }
    log.entered(TOKEN, base() + 4 * HOUR, None);
    log.game(sealed_match("b0", base() + 5 * HOUR));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 2);
    assert_eq!(tokens(&fold.runs[0]), vec!["a0", "a1", "a2"]);
    assert_eq!(fold.runs[0].state, RunState::Partial, "three of five seen");
    assert_eq!(tokens(&fold.runs[1]), vec!["b0"]);
    assert_eq!(fold.runs[1].state, RunState::Active);
}

#[test]
fn entering_another_league_leaves_this_run_alone() {
    let mut log = Log::default();
    log.entered(TOKEN, base() - MINUTE, None);
    log.game(sealed_match("a0", base()));
    log.entered(TOKEN_2, base() + HOUR, None);
    log.game(sealed_match("a1", base() + 2 * HOUR));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(tokens(&fold.runs[0]), vec!["a0", "a1"]);
    assert_eq!(fold.runs[0].state, RunState::Active);
}

#[test]
fn a_join_request_without_its_panel_closes_nothing() {
    let mut log = Log::default();
    log.entered(TOKEN, base() - MINUTE, None);
    log.game(sealed_match("a0", base()));
    log.push(
        base() + HOUR,
        LeagueInput::JoinRequested {
            at_ms: base() + HOUR,
        },
    );
    log.joined(TOKEN, EVENT, base() + 2 * HOUR);
    log.game(sealed_match("a1", base() + 3 * HOUR));
    let fold = log.fold();
    assert_eq!(
        fold.runs.len(),
        1,
        "the panel came an hour later: a view, not an entry"
    );
    assert_eq!(tokens(&fold.runs[0]), vec!["a0", "a1"]);
}

#[test]
fn a_panel_for_another_league_after_the_window_is_a_view_and_closes_nothing() {
    let mut log = Log::default();
    log.entered(TOKEN, base() - MINUTE, None);
    log.game(sealed_match("a0", base()));
    log.push(
        base() + HOUR,
        LeagueInput::JoinRequested {
            at_ms: base() + HOUR,
        },
    );
    log.joined(TOKEN_2, EVENT, base() + HOUR + 10 * SECOND);
    log.joined(TOKEN, EVENT, base() + HOUR + 20 * SECOND);
    log.game(sealed_match("a1", base() + 2 * HOUR));
    let fold = log.fold();
    assert_eq!(
        fold.runs.len(),
        1,
        "neither panel was logged with the request"
    );
    assert_eq!(tokens(&fold.runs[0]), vec!["a0", "a1"]);
    assert_eq!(fold.runs[0].state, RunState::Active);
}

#[test]
fn a_league_stating_six_matches_runs_six() {
    let mut log = Log::default();
    log.entered(TOKEN, base() - MINUTE, Some(6));
    for index in 0..7 {
        log.game(sealed_match(&format!("s{index}"), base() + index * HOUR));
    }
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 2);
    assert_eq!(fold.runs[0].round_count(), 6);
    assert_eq!(
        tokens(&fold.runs[0]),
        vec!["s0", "s1", "s2", "s3", "s4", "s5"]
    );
    assert_eq!(fold.runs[0].state, RunState::Complete);
    assert_eq!(tokens(&fold.runs[1]), vec!["s6"]);
}

#[test]
fn a_run_without_a_stated_length_takes_its_kinds() {
    let mut log = Log::default();
    log.game(sealed_match("s0", base()));
    let fold = log.fold();
    assert_eq!(fold.runs[0].rounds, None);
    assert_eq!(fold.runs[0].round_count(), 5);
}

#[test]
fn a_draft_run_completes_at_its_third_complete_match() {
    let mut log = Log::default();
    for index in 0..3 {
        log.game(draft_match(&format!("d{index}"), base() + index * HOUR));
    }
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.kind, LeagueKind::Draft);
    assert_eq!(run.set_code.as_deref(), Some("HOB"));
    assert_eq!(run.state, RunState::Complete);
    assert_eq!(run.completed_at, Some(base() + 2 * HOUR + 20 * MINUTE));
}

#[test]
fn a_constructed_run_turns_limited_when_a_limited_match_joins_and_never_back() {
    let mut log = Log::default();
    log.game(match_in(None, "m1", base()));
    log.game(sealed_match("m2", base() + HOUR));
    log.game(match_in(Some("CMODERN"), "m3", base() + 2 * HOUR));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(tokens(run), vec!["m1", "m2", "m3"]);
    assert_eq!(run.kind, LeagueKind::Sealed);
    assert_eq!(run.set_code.as_deref(), Some("FRA"));
}

#[test]
fn fold_runs_alone_still_leaves_limited_matches_and_drafts_out() {
    let mut log = Log::default();
    log.game(draft_match("d1", base()));
    log.draft(draft(DRAFT_TOKEN, Some(EVENT), Some(COURSE), 1..=3, base()));
    assert!(fold_runs(log.items.clone(), UTC).is_empty());
    assert_eq!(log.fold().runs.len(), 2);
}

#[test]
fn constructed_runs_fold_the_same_with_or_without_the_limited_rules() {
    let mut log = Log::default();
    log.joined(TOKEN_2, 500, base() - MINUTE);
    for index in 0..7 {
        log.game(on_token(
            with_version(
                match_in(Some("CMODERN"), &format!("c{index}"), base() + index * HOUR),
                1 + index / 4,
            ),
            TOKEN_2,
        ));
    }
    log.game(draft_match("d1", base() + 30 * MINUTE));
    // The paths that touch shared state: a draft, a constructed 0.x seed,
    // and a drop after a limited league's panel view.
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    let mut members = vec![
        on_token(match_in(Some("CMODERN"), "x1", base() + DAY), "q-seed"),
        on_token(
            match_in(Some("CMODERN"), "x2", base() + DAY + HOUR),
            "q-seed",
        ),
    ];
    members[1].state = MatchState::InProgress;
    log.push(
        base() + DAY,
        LeagueInput::ImportedRun(ImportedRunSeed {
            key: "0x:77".to_string(),
            token: "q-seed".to_string(),
            event_id: Some(900),
            joined_at: None,
            name: "Qimported League".to_string(),
            format: Some("CMODERN".to_string()),
            deck_version_id: None,
            started_at: base() + DAY,
            state: RunState::Active,
            dropped_at: None,
            completed_at: None,
            matches: members,
        }),
    );
    log.joined(TOKEN, EVENT, base() + 2 * DAY);
    log.push(
        base() + 2 * DAY + MINUTE,
        LeagueInput::Dropped {
            at_ms: base() + 2 * DAY + MINUTE,
        },
    );
    let constructed = fold_runs(log.items.clone(), UTC);
    let both: Vec<Run> = log
        .fold()
        .runs
        .into_iter()
        .filter(|run| run.kind == LeagueKind::Constructed)
        .collect();
    assert_eq!(constructed, both);
    assert_eq!(constructed.len(), 3);
    assert!(constructed.iter().all(|run| run.state != RunState::Dropped));
    // The drop lands on the limited league's newest run (the panel view
    // gave `d1`'s run its event id) in the limited fold only.
    assert_eq!(run_of(&log.fold(), "d1").state, RunState::Dropped);
}

#[test]
fn a_format_less_match_then_a_limited_one_on_its_token_split_between_the_folds() {
    // A join block without `PlayFormatCd` (see the traps doc): the
    // constructed fold keeps the first match alone in a constructed run;
    // the limited fold upgrades that run with the limited match.
    let mut log = Log::default();
    log.game(match_in(None, "m1", base()));
    log.game(draft_match("m2", base() + HOUR));
    let constructed = fold_runs(log.items.clone(), UTC);
    assert_eq!(constructed.len(), 1);
    assert_eq!(tokens(&constructed[0]), vec!["m1"]);
    assert_eq!(constructed[0].kind, LeagueKind::Constructed);
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(tokens(&fold.runs[0]), vec!["m1", "m2"]);
    assert_eq!(fold.runs[0].kind, LeagueKind::Draft);
}

// 0.x AssignLeagueTest: the limited re-entry pool guard

/// A draft run on `TOKEN` for `EVENT` holding a finished draft of
/// 1000..=1022, with a panel view first so it takes the real token.
fn draft_run_log() -> Log {
    let mut log = Log::default();
    log.joined(TOKEN, EVENT, base() - MINUTE);
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1000..=1022,
        base(),
    ));
    log
}

#[test]
fn a_registered_deck_that_does_not_fit_the_pool_mints_a_new_draft_run() {
    let mut log = draft_run_log();
    log.known(5000..=5022, None);
    log.game(draft_match("m1", base() + HOUR));
    log.registered("m1", main_deck(5000..=5022), base() + HOUR);
    let fold = log.fold();

    let on_token: Vec<&Run> = fold.runs.iter().filter(|run| run.token == TOKEN).collect();
    assert_eq!(on_token.len(), 2);
    let drafted = run_with_draft(&fold, DRAFT_TOKEN);
    assert_eq!(drafted.state, RunState::Partial);
    assert!(drafted.match_tokens.is_empty());
    let fresh = run_of(&fold, "m1");
    assert_ne!(fresh.key, drafted.key);
    assert_eq!(fresh.kind, LeagueKind::Draft);
    assert_eq!(fresh.token, TOKEN);
}

#[test]
fn a_registered_deck_that_fits_the_pool_joins_the_draft_run() {
    let mut log = draft_run_log();
    log.known(1000..=1022, None);
    log.game(draft_match("m1", base() + HOUR));
    log.registered("m1", main_deck(1000..=1022), base() + HOUR);
    let fold = log.fold();

    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.key, format!("draft:{DRAFT_TOKEN}"));
    assert_eq!(tokens(run), vec!["m1"]);
    assert_eq!(run.state, RunState::Active);
}

#[test]
fn the_pool_guard_leaves_constructed_runs_alone() {
    let mut log = Log::default();
    log.game(match_in(Some("CSTANDARD"), "c1", base()));
    log.game(match_in(Some("CSTANDARD"), "c2", base() + HOUR));
    log.registered(
        "c2",
        vec![RegisteredCard {
            catalog_id: 9999,
            quantity: 4,
            sideboard: false,
        }],
        base() + HOUR,
    );
    log.known([9999], None);
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(tokens(&fold.runs[0]), vec!["c1", "c2"]);
    assert_eq!(fold.runs[0].state, RunState::Active);
}

#[test]
fn an_unfinished_draft_never_rejects_a_deck() {
    let mut log = Log::default();
    log.joined(TOKEN, EVENT, base() - MINUTE);
    log.draft(picking(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1000..=1011,
        base(),
    )));
    log.known(5000..=5022, None);
    log.game(draft_match("m1", base() + HOUR));
    log.registered("m1", main_deck(5000..=5022), base() + HOUR);
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(tokens(&fold.runs[0]), vec!["m1"]);
    assert_eq!(fold.runs[0].state, RunState::Active);
}

#[test]
fn a_draft_with_no_picks_never_rejects_a_deck() {
    let mut log = Log::default();
    log.joined(TOKEN, EVENT, base() - MINUTE);
    log.draft(draft(DRAFT_TOKEN, Some(EVENT), Some(COURSE), [], base()));
    log.known(5000..=5022, None);
    log.game(draft_match("m1", base() + HOUR));
    log.registered("m1", main_deck(5000..=5022), base() + HOUR);
    assert_eq!(tokens(&log.fold().runs[0]), vec!["m1"]);
}

#[test]
fn basic_lands_and_unknown_cards_are_neutral_in_the_pool_guard() {
    let mut log = draft_run_log();
    log.known(1000..=1022, None);
    // Seventeen basics and 23 cards with no card data: neither side counts.
    log.facts.cards.insert(
        1,
        CardFacts {
            card_type: Some("Basic Land".to_string()),
            ..CardFacts::default()
        },
    );
    let mut cards = main_deck(1000..=1005);
    cards.push(RegisteredCard {
        catalog_id: 1,
        quantity: 17,
        sideboard: false,
    });
    cards.extend(main_deck(7000..=7022));
    log.game(draft_match("m1", base() + HOUR));
    log.registered("m1", cards, base() + HOUR);
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(tokens(&fold.runs[0]), vec!["m1"]);
}

#[test]
fn a_limited_run_keeps_its_matches_across_deck_version_changes() {
    let mut log = Log::default();
    log.joined(TOKEN, EVENT, base() - MINUTE);
    log.game(with_version(draft_match("m1", base()), 1));
    log.draft_at(
        draft(DRAFT_TOKEN, Some(EVENT), None, 1000..=1022, base() - HOUR),
        base() + 30 * MINUTE,
    );
    log.known(1000..=1022, None);
    log.game(with_version(draft_match("m2", base() + HOUR), 2));
    log.registered("m2", main_deck(1000..=1022), base() + HOUR);
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(tokens(run), vec!["m1", "m2"]);
    assert_eq!(run.deck_version_id, Some(1));
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));
}

#[test]
fn a_constructed_run_still_splits_on_a_deck_version_change() {
    let mut log = Log::default();
    log.game(with_version(match_in(Some("CSTANDARD"), "c1", base()), 1));
    log.game(with_version(
        match_in(Some("CSTANDARD"), "c2", base() + HOUR),
        2,
    ));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 2);
    assert_eq!(fold.runs[0].state, RunState::Partial);
    assert_eq!(tokens(&fold.runs[1]), vec!["c2"]);
}

#[test]
fn a_limited_run_never_adopts_a_deck_version() {
    let mut log = draft_run_log();
    log.game(with_version(draft_match("m1", base() + HOUR), 5));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(tokens(&fold.runs[0]), vec!["m1"]);
    assert_eq!(fold.runs[0].deck_version_id, None);
}

// 0.x AssignLeagueTest: placeholder token healing, and ResolveDraftLeagueTest

#[test]
fn a_draft_before_any_panel_view_mints_a_placeholder_run() {
    let mut log = Log::default();
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    log.draft(draft(
        DRAFT_TOKEN_2,
        Some(EVENT + 1),
        None,
        1..=42,
        base() + DAY,
    ));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 2);
    let run = &fold.runs[0];
    assert_eq!(run.key, format!("draft:{DRAFT_TOKEN}"));
    assert_eq!(run.token, "draft-11039-35746768");
    assert_eq!(run.kind, LeagueKind::Draft);
    assert_eq!(run.name, "Draft League 11039");
    assert_eq!(run.format.as_deref(), Some("Limited"));
    assert_eq!(run.state, RunState::Active);
    assert_eq!(run.event_id, Some(EVENT));
    assert_eq!(run.course_id, Some(COURSE as i64));
    assert_eq!(run.started_at, base());
    assert_eq!(run.joined_at, Some(base()));
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));
    assert_eq!(fold.runs[1].token, "draft-11040-unknown");
    assert!(fold.unlinked_drafts.is_empty());
}

#[test]
fn a_draft_takes_the_token_of_the_latest_panel_view_of_its_league() {
    let mut log = Log::default();
    log.joined(TOKEN_2, EVENT, base() - HOUR);
    log.joined(TOKEN, EVENT, base() - MINUTE);
    log.joined(TOKEN_2, EVENT + 1, base() - SECOND);
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    let fold = log.fold();
    assert_eq!(fold.runs[0].token, TOKEN);
    assert_eq!(fold.runs[0].set_code.as_deref(), Some("HOB"));
}

#[test]
fn a_placeholder_token_heals_when_a_match_follows_a_panel_view() {
    let mut log = Log::default();
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    log.joined(TOKEN, EVENT, base() + 30 * MINUTE);
    log.game(draft_match("m1", base() + HOUR));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.token, TOKEN);
    assert_eq!(run.key, format!("draft:{DRAFT_TOKEN}"));
    assert_eq!(tokens(run), vec!["m1"]);
}

#[test]
fn a_placeholder_run_whose_pool_fails_the_deck_is_not_healed() {
    let mut log = Log::default();
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1000..=1022,
        base(),
    ));
    log.known(5000..=5022, None);
    log.joined(TOKEN, EVENT, base() + 30 * MINUTE);
    log.game(draft_match("m1", base() + HOUR));
    log.registered("m1", main_deck(5000..=5022), base() + HOUR);
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 2);
    let drafted = run_with_draft(&fold, DRAFT_TOKEN);
    assert_eq!(drafted.token, "draft-11039-35746768");
    assert!(drafted.match_tokens.is_empty());
    assert_ne!(run_of(&fold, "m1").key, drafted.key);
}

#[test]
fn a_real_token_is_never_overwritten() {
    let mut log = Log::default();
    log.joined(TOKEN, EVENT, base() - MINUTE);
    log.game(draft_match("m1", base()));
    log.joined(TOKEN_2, EVENT, base() + 30 * MINUTE);
    log.draft_at(
        draft(DRAFT_TOKEN, Some(EVENT), None, 1..=42, base() - HOUR),
        base() + HOUR,
    );
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.token, TOKEN);
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));
}

#[test]
fn a_stale_draft_run_rejected_by_the_pool_completes_when_it_has_its_matches() {
    let mut log = Log::default();
    log.joined(TOKEN, EVENT, base() - MINUTE);
    for index in 0..3 {
        log.game(unfinished(draft_match(
            &format!("m{index}"),
            base() + index * HOUR,
        )));
    }
    log.draft_at(
        draft(DRAFT_TOKEN, Some(EVENT), None, 1000..=1022, base() - HOUR),
        base() + 3 * HOUR,
    );
    log.known(5000..=5022, None);
    log.game(draft_match("m3", base() + 4 * HOUR));
    log.registered("m3", main_deck(5000..=5022), base() + 4 * HOUR);
    let fold = log.fold();
    let stale = run_of(&fold, "m0");
    assert_eq!(stale.state, RunState::Complete);
    assert_eq!(stale.completed_at, Some(base() + 4 * HOUR));
    assert_eq!(stale.draft_token.as_deref(), Some(DRAFT_TOKEN));
    assert_ne!(run_of(&fold, "m3").key, stale.key);
}

#[test]
fn a_new_draft_closes_the_older_active_runs_of_its_league() {
    // Two matches: not a full run, so the older run is Partial.
    let mut log = draft_run_log();
    log.game(draft_match("m1", base() + HOUR));
    log.game(draft_match("m2", base() + 2 * HOUR));
    log.draft(draft(
        DRAFT_TOKEN_2,
        Some(EVENT),
        Some(COURSE + 1),
        1..=42,
        base() + DAY,
    ));
    let fold = log.fold();
    assert_eq!(of_event(&fold, EVENT).len(), 2);
    assert_eq!(run_with_draft(&fold, DRAFT_TOKEN).state, RunState::Partial);
    let newer = run_with_draft(&fold, DRAFT_TOKEN_2);
    assert_eq!(newer.state, RunState::Active);
    assert_eq!(newer.token, TOKEN);

    // Three matches that never completed: a full run, so Complete.
    let mut log = draft_run_log();
    for index in 1..=3 {
        log.game(unfinished(draft_match(
            &format!("m{index}"),
            base() + index * HOUR,
        )));
    }
    log.draft(draft(
        DRAFT_TOKEN_2,
        Some(EVENT),
        Some(COURSE + 1),
        1..=42,
        base() + DAY,
    ));
    let fold = log.fold();
    let older = run_with_draft(&fold, DRAFT_TOKEN);
    assert_eq!(older.state, RunState::Complete);
    assert_eq!(older.completed_at, Some(base() + DAY));
}

#[test]
fn a_draft_rejoins_the_run_of_its_course() {
    let mut log = Log::default();
    log.imported(
        ImportedRunSeed {
            key: "0x:29".to_string(),
            token: TOKEN.to_string(),
            event_id: Some(EVENT),
            joined_at: Some(base()),
            name: "Draft League 11039".to_string(),
            format: Some("Limited".to_string()),
            deck_version_id: None,
            started_at: base(),
            state: RunState::Active,
            dropped_at: None,
            completed_at: None,
            matches: Vec::new(),
        },
        LimitedSeed {
            kind: LeagueKind::Draft,
            set_code: Some("HOB".to_string()),
            course_id: Some(COURSE as i64),
        },
    );
    log.draft_at(
        draft(DRAFT_TOKEN, Some(EVENT), Some(COURSE), 1..=42, base()),
        base() + MINUTE,
    );
    log.game(draft_match("m1", base() + HOUR));
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.key, "0x:29");
    assert_eq!(run.kind, LeagueKind::Draft);
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));
    assert_eq!(tokens(run), vec!["m1"]);
}

#[test]
fn an_imported_draft_stays_with_the_run_0x_linked_it_to() {
    let mut log = Log::default();
    log.imported(
        ImportedRunSeed {
            key: "0x:29".to_string(),
            token: TOKEN.to_string(),
            event_id: None,
            joined_at: None,
            name: "Draft League 11039".to_string(),
            format: Some("Limited".to_string()),
            deck_version_id: None,
            started_at: base(),
            state: RunState::Complete,
            dropped_at: None,
            completed_at: Some(base() + HOUR),
            matches: vec![draft_match("x1", base())],
        },
        LimitedSeed {
            kind: LeagueKind::Sealed,
            set_code: None,
            course_id: None,
        },
    );
    log.imported_draft(draft(DRAFT_TOKEN, None, None, 1..=42, base()), "0x:29");
    log.imported_draft(draft(DRAFT_TOKEN_2, None, None, 1..=5, base()), "0x:404");
    let fold = log.fold();
    assert_eq!(fold.runs.len(), 1);
    let run = &fold.runs[0];
    assert_eq!(run.kind, LeagueKind::Sealed);
    assert_eq!(run.state, RunState::Complete);
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));
    assert_eq!(fold.unlinked_drafts, vec![DRAFT_TOKEN_2.to_string()]);
}

// 0.x LinkUnlinkedDraftsTest: late linking by pool

/// A draft run on `token` minted by match `match_token`, whose registered
/// deck is `deck`.
fn draftless_run(log: &mut Log, token: &str, match_token: &str, deck: Vec<RegisteredCard>) {
    log.game(on_token(draft_match(match_token, base() + HOUR), token));
    log.registered(match_token, deck, base() + HOUR);
}

#[test]
fn a_draft_with_no_league_links_to_the_run_its_registered_deck_came_from() {
    let mut log = Log::default();
    log.known(1000..=1022, None);
    draftless_run(&mut log, TOKEN, "m1", main_deck(1000..=1022));
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()));
    let fold = log.fold();
    assert_eq!(
        run_of(&fold, "m1").draft_token.as_deref(),
        Some(DRAFT_TOKEN)
    );
    assert!(fold.unlinked_drafts.is_empty());
}

#[test]
fn a_draft_stays_unlinked_when_no_registered_deck_fits() {
    let mut log = Log::default();
    log.known(5000..=5022, None);
    draftless_run(&mut log, TOKEN, "m1", main_deck(5000..=5022));
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()));
    let fold = log.fold();
    assert_eq!(run_of(&fold, "m1").draft_token, None);
    assert_eq!(fold.unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
}

#[test]
fn a_draft_stays_unlinked_from_another_pool_even_without_card_data() {
    let mut log = Log::default();
    draftless_run(&mut log, TOKEN, "m1", main_deck(5000..=5022));
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()));
    assert_eq!(log.fold().unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
}

#[test]
fn a_draft_links_on_cards_without_data_when_the_deck_was_built_from_its_pool() {
    let mut log = Log::default();
    log.stubs(1000..=1022);
    draftless_run(&mut log, TOKEN, "m1", main_deck(1000..=1022));
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()));
    let fold = log.fold();
    assert_eq!(
        run_of(&fold, "m1").draft_token.as_deref(),
        Some(DRAFT_TOKEN)
    );
}

#[test]
fn two_unlinked_drafts_each_link_to_their_own_run_in_any_order() {
    let mut log = Log::default();
    log.known(1000..=1022, None);
    log.known(2000..=2022, None);
    draftless_run(&mut log, TOKEN, "a1", main_deck(1000..=1022));
    draftless_run(&mut log, TOKEN_2, "b1", main_deck(2000..=2022));
    log.draft(draft(DRAFT_TOKEN_2, None, None, 2000..=2041, base()));
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base() + MINUTE));
    let fold = log.fold();
    assert_eq!(
        run_of(&fold, "a1").draft_token.as_deref(),
        Some(DRAFT_TOKEN)
    );
    assert_eq!(
        run_of(&fold, "b1").draft_token.as_deref(),
        Some(DRAFT_TOKEN_2)
    );
}

#[test]
fn a_draft_does_not_link_to_a_run_of_another_set() {
    let mut log = Log::default();
    log.known(1000..=1022, Some("HOB"));
    log.game(match_in(Some("DMSHMSHMSH"), "m1", base() + HOUR));
    log.registered("m1", main_deck(1000..=1022), base() + HOUR);
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()));
    let fold = log.fold();
    assert_eq!(run_of(&fold, "m1").set_code.as_deref(), Some("MSH"));
    assert_eq!(fold.unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
}

#[test]
fn a_draft_does_not_link_to_a_run_started_more_than_a_week_apart() {
    let mut log = Log::default();
    log.known(1000..=1022, None);
    log.game(draft_match("m1", base() - 10 * DAY));
    log.registered("m1", main_deck(1000..=1022), base() - 10 * DAY);
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()));
    assert_eq!(log.fold().unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
}

#[test]
fn a_draft_0x_linked_never_late_links_to_another_run() {
    let mut log = Log::default();
    log.known(1000..=1022, None);
    draftless_run(&mut log, TOKEN, "m1", main_deck(1000..=1022));
    log.imported(
        ImportedRunSeed {
            key: "0x:29".to_string(),
            token: TOKEN_2.to_string(),
            event_id: None,
            joined_at: None,
            name: "Draft League 11039".to_string(),
            format: Some("Limited".to_string()),
            deck_version_id: None,
            started_at: base(),
            state: RunState::Complete,
            dropped_at: None,
            completed_at: Some(base()),
            matches: Vec::new(),
        },
        LimitedSeed {
            kind: LeagueKind::Draft,
            set_code: None,
            course_id: None,
        },
    );
    // The seed's own draft, a second one 0.x put on the same league, and
    // one whose run was never imported: both fit the draftless run.
    log.imported_draft(draft("q-draft-own", None, None, 1..=42, base()), "0x:29");
    log.imported_draft(draft(DRAFT_TOKEN, None, None, 1000..=1041, base()), "0x:29");
    log.imported_draft(
        draft(DRAFT_TOKEN_2, None, None, 1000..=1041, base()),
        "0x:404",
    );
    let fold = log.fold();
    assert_eq!(run_of(&fold, "m1").draft_token, None);
    assert_eq!(
        fold.unlinked_drafts,
        vec![DRAFT_TOKEN.to_string(), DRAFT_TOKEN_2.to_string()]
    );
}

#[test]
fn a_draft_with_fewer_than_twenty_picks_is_not_evidence_for_a_link() {
    let mut log = Log::default();
    log.known(1000..=1022, None);
    draftless_run(&mut log, TOKEN, "m1", main_deck(1000..=1022));
    log.draft(draft(DRAFT_TOKEN, None, None, 1000..=1018, base()));
    assert_eq!(log.fold().unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
}

// 0.x AdoptCourselessDraftLeagueTest

/// A draft whose lines named an older real token for its league, so it
/// mints its own run, and the run's matches arrive on the newer token and
/// mint a course-less run: the 0.x "app not watching the draft" split.
fn split_run(deck: Vec<RegisteredCard>, match_at: i64) -> Log {
    let mut log = Log::default();
    log.joined(TOKEN_2, EVENT, base() - 4 * HOUR);
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1000..=1041,
        base() - 3 * HOUR,
    ));
    log.joined(TOKEN, EVENT, match_at - MINUTE);
    log.game(draft_match("m1", match_at));
    log.registered("m1", deck, match_at);
    log
}

#[test]
fn the_course_less_run_holding_the_matches_adopts_the_draft() {
    let mut log = split_run(main_deck(1000..=1022), base() - 2 * HOUR);
    log.known(1000..=1022, None);
    let fold = log.fold();
    let runs = of_event(&fold, EVENT);
    assert_eq!(runs.len(), 1);
    let run = runs[0];
    assert_eq!(tokens(run), vec!["m1"]);
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));
    assert_eq!(run.course_id, Some(COURSE as i64));
    assert_eq!(run.state, RunState::Active);
    assert_eq!(run.started_at, base() - 3 * HOUR);
    assert!(fold.unlinked_drafts.is_empty());
}

#[test]
fn an_adopted_run_with_its_full_run_played_completes() {
    let mut log = split_run(main_deck(1000..=1022), base() - 2 * HOUR);
    log.known(1000..=1022, None);
    log.game(draft_match("m2", base() - HOUR));
    log.game(draft_match("m3", base()));
    let fold = log.fold();
    let runs = of_event(&fold, EVENT);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Complete);
    assert_eq!(runs[0].completed_at, Some(base() + 20 * MINUTE));
}

#[test]
fn the_adopting_run_answers_to_the_draft_runs_key_and_keeps_its_drop() {
    let draft_key = format!("draft:{DRAFT_TOKEN}");
    // Dropped by hand while it was the draft's run.
    let mut log = split_run(main_deck(1000..=1022), base() - 2 * HOUR);
    log.known(1000..=1022, None);
    log.manual_drop(&draft_key, base() - 150 * MINUTE);
    let fold = log.fold();
    let runs = of_event(&fold, EVENT);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].key, format!("{TOKEN}:m1"));
    assert_eq!(runs[0].former_keys, vec![draft_key.clone()]);
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() - 150 * MINUTE));

    // A drop stamped before the draft run existed still finds it.
    let mut log = split_run(main_deck(1000..=1022), base() - 2 * HOUR);
    log.known(1000..=1022, None);
    log.manual_drop(&draft_key, base() - 5 * HOUR);
    let runs: Vec<Run> = log
        .fold()
        .runs
        .into_iter()
        .filter(|run| run.event_id == Some(EVENT))
        .collect();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].state, RunState::Dropped);
    assert_eq!(runs[0].dropped_at, Some(base() - 5 * HOUR));
}

#[test]
fn both_runs_stay_when_the_registered_deck_is_another_pool() {
    let mut log = split_run(main_deck(5000..=5022), base() - 2 * HOUR);
    log.known(5000..=5022, None);
    let fold = log.fold();
    assert_eq!(of_event(&fold, EVENT).len(), 2);
    assert!(run_with_draft(&fold, DRAFT_TOKEN).match_tokens.is_empty());
    assert_eq!(run_of(&fold, "m1").course_id, None);
}

#[test]
fn a_draft_run_that_already_holds_matches_adopts_nothing() {
    let mut log = split_run(main_deck(1000..=1022), base() - 2 * HOUR);
    log.known(1000..=1022, None);
    log.game(on_token(draft_match("own", base() - 150 * MINUTE), TOKEN_2));
    let fold = log.fold();
    assert_eq!(of_event(&fold, EVENT).len(), 2);
    assert_eq!(tokens(run_with_draft(&fold, DRAFT_TOKEN)), vec!["own"]);
    assert_eq!(run_of(&fold, "m1").course_id, None);
}

#[test]
fn a_run_whose_matches_started_before_the_draft_is_not_adopted() {
    let mut log = split_run(main_deck(1000..=1022), base() - 3 * HOUR - DAY);
    log.known(1000..=1022, None);
    let fold = log.fold();
    assert_eq!(of_event(&fold, EVENT).len(), 2);
    assert_eq!(run_of(&fold, "m1").course_id, None);
}

// Drops and order independence

#[test]
fn a_draft_run_can_be_dropped_by_its_key() {
    let mut log = draft_run_log();
    log.manual_drop(&format!("draft:{DRAFT_TOKEN}"), base() + HOUR);
    let fold = log.fold();
    assert_eq!(fold.runs[0].state, RunState::Dropped);
    assert_eq!(fold.runs[0].dropped_at, Some(base() + HOUR));
}

#[test]
fn shuffled_limited_inputs_give_the_same_fold() {
    let mut log = split_run(main_deck(1000..=1022), base() - 2 * HOUR);
    log.known(1000..=1022, None);
    log.game(draft_match("m2", base() - HOUR));
    log.draft(draft(
        DRAFT_TOKEN_2,
        Some(EVENT),
        Some(COURSE + 1),
        1..=42,
        base() + DAY,
    ));
    log.game(sealed_match("s1", base() + 2 * DAY));
    log.draft(draft(
        DRAFT_TOKEN_2.replace('a', "b").as_str(),
        None,
        None,
        1..=3,
        base(),
    ));
    let want = log.fold();

    let mut reversed = log.clone();
    reversed.items.reverse();
    reversed.facts.drafts.reverse();
    assert_eq!(reversed.fold(), want);

    let mut rotated = log.clone();
    rotated.items.rotate_left(3);
    rotated.facts.drafts.rotate_left(1);
    assert_eq!(rotated.fold(), want);
}

// 0.x FormatLeagueRuns::classifyRun for limited runs

fn complete_match(index: i64, outcome: Outcome) -> RunMatch {
    RunMatch {
        token: format!("m{index}"),
        state: MatchState::Complete,
        outcome,
        started_at: base() + index * HOUR,
        ended_at: Some(base() + index * HOUR + 20 * MINUTE),
        deck_version_id: None,
        deck_id: None,
        games: Vec::new(),
        opponent_archetype: None,
    }
}

fn limited_run(state: RunState, outcomes: &[Outcome]) -> RunInput {
    RunInput {
        course_record: None,
        state,
        manual: false,
        uncapped: false,
        format: Some("Limited".to_string()),
        started_at: base(),
        matches: outcomes
            .iter()
            .enumerate()
            .map(|(index, outcome)| complete_match(index as i64, *outcome))
            .collect(),
    }
}

#[test]
fn a_limited_run_is_classified_against_its_own_round_count() {
    let classify = |kind: LeagueKind, state: RunState, outcomes: &[Outcome]| {
        summarize_limited_run(&limited_run(state, outcomes), kind.round_count(), UTC).classification
    };
    let draft = LeagueKind::Draft;
    let sealed = LeagueKind::Sealed;
    let complete = RunState::Complete;
    assert_eq!(
        classify(draft, complete, &[Outcome::Win, Outcome::Win, Outcome::Win]),
        Classification::Trophy
    );
    assert_eq!(
        classify(
            draft,
            complete,
            &[Outcome::Win, Outcome::Loss, Outcome::Win]
        ),
        Classification::Cash
    );
    assert_eq!(
        classify(
            draft,
            complete,
            &[Outcome::Loss, Outcome::Loss, Outcome::Win]
        ),
        Classification::Finish
    );
    assert_eq!(
        classify(sealed, complete, &[Outcome::Win; 5]),
        Classification::Trophy
    );
    assert_eq!(
        classify(
            sealed,
            complete,
            &[
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Loss
            ]
        ),
        Classification::Cash
    );
    assert_eq!(
        classify(
            sealed,
            complete,
            &[Outcome::Win, Outcome::Win, Outcome::Win]
        ),
        Classification::Finish
    );
    assert_eq!(
        classify(draft, RunState::Dropped, &[Outcome::Win]),
        Classification::Brick
    );

    let live = summarize_limited_run(
        &limited_run(RunState::Active, &[Outcome::Win]),
        draft.round_count(),
        UTC,
    );
    assert_eq!(live.classification, Classification::Live);
    assert_eq!(live.live_round, Some(2));
    assert_eq!(live.results, vec![Some(Mark::W), None, None]);
    let sealed_live = summarize_limited_run(
        &limited_run(RunState::Active, &[Outcome::Loss]),
        sealed.round_count(),
        UTC,
    );
    assert_eq!(sealed_live.results.len(), 5);
    // Limited formats are not priced.
    let finished = summarize_limited_run(
        &limited_run(complete, &[Outcome::Win, Outcome::Win, Outcome::Win]),
        draft.round_count(),
        UTC,
    );
    assert_eq!(finished.tix_delta, None);
}

#[test]
fn a_run_of_a_six_match_league_is_summarized_against_six() {
    let mut outcomes = [Outcome::Win; 6];
    outcomes[5] = Outcome::Loss;
    let six = summarize_limited_run(
        &limited_run(RunState::Complete, &outcomes),
        LeagueKind::Sealed.round_count_stated(Some(6)),
        UTC,
    );
    assert_eq!(six.classification, Classification::Cash, "5-1 of six");
    assert_eq!(six.results.len(), 6);
    let live = summarize_limited_run(
        &limited_run(RunState::Active, &[Outcome::Win]),
        LeagueKind::Sealed.round_count_stated(Some(6)),
        UTC,
    );
    assert_eq!(live.results.len(), 6);
}

#[test]
fn an_uncapped_run_has_no_padding_and_no_trophy() {
    let mut complete = limited_run(RunState::Complete, &[Outcome::Win; 7]);
    complete.uncapped = true;
    let finished = summarize_limited_run(&complete, LeagueKind::Draft.round_count(), UTC);
    assert_eq!(finished.classification, Classification::Finish);
    assert_eq!(finished.results, vec![Some(Mark::W); 7]);

    let mut active = limited_run(RunState::Active, &[Outcome::Win, Outcome::Loss]);
    active.uncapped = true;
    let live = summarize_limited_run(&active, LeagueKind::Sealed.round_count(), UTC);
    assert_eq!(live.classification, Classification::Live);
    assert_eq!(live.live_round, Some(3));
    assert_eq!(live.results, vec![Some(Mark::W), Some(Mark::L)]);
}

#[test]
fn a_limited_run_counts_its_complete_matches_without_deck_links() {
    let mut run = limited_run(
        RunState::Complete,
        &[Outcome::Win, Outcome::Loss, Outcome::Win],
    );
    run.matches[2].state = MatchState::Abandoned;
    let summary = summarize_limited_run(&run, LeagueKind::Draft.round_count(), UTC);
    assert_eq!(summary.results, vec![Some(Mark::W), Some(Mark::L)]);
    assert_eq!(summary.matches.len(), 2);
}

/// A Vintage Cube league match (`DC03C03C03`, `Vintage Cube x3`).
fn cube_match(token: &str, started_at: i64) -> LeagueMatch {
    let mut played = match_in(Some("DC03C03C03"), token, started_at);
    played.game_structure = Some("Vintage Cube x3".to_string());
    played
}

/// A cube draft: its picks span many sets, `PRM` the most.
fn cube_log() -> Log {
    let mut log = Log::default();
    log.known(1..=20, Some("PRM"));
    for id in 21..=42 {
        let set = if id % 2 == 0 { "EXO" } else { "MH3" };
        log.known([id], Some(set));
    }
    log
}

#[test]
fn a_cube_draft_takes_its_set_code_from_the_matches_not_the_picks() {
    let mut log = cube_log();
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    log.items.push((
        base() + 30 * MINUTE,
        1,
        log.next_offset(),
        LeagueInput::Joined {
            token: TOKEN.to_string(),
            event_id: EVENT,
            at_ms: base() + 30 * MINUTE,
            rounds: None,
        },
    ));
    log.game(cube_match("m1", base() + HOUR));
    let fold = log.fold();
    let run = run_with_draft(&fold, DRAFT_TOKEN);
    assert_eq!(tokens(run), vec!["m1"]);
    assert_eq!(run.set_code.as_deref(), Some("C03"));
}

#[test]
fn a_cube_panel_view_names_the_set_before_any_match() {
    let mut log = cube_log();
    log.items.push((
        base() - MINUTE,
        1,
        0,
        LeagueInput::Joined {
            token: TOKEN.to_string(),
            event_id: EVENT,
            at_ms: base() - MINUTE,
            rounds: None,
        },
    ));
    log.facts.panels.push(PanelFormat {
        event_id: EVENT,
        at_ms: base() - MINUTE,
        play_format: "DC03C03C03".to_string(),
    });
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    let fold = log.fold();
    assert_eq!(fold.runs[0].set_code.as_deref(), Some("C03"));
}

#[test]
fn a_picks_guess_stands_when_no_format_code_names_the_set() {
    let mut log = cube_log();
    log.draft(draft(
        DRAFT_TOKEN,
        Some(EVENT),
        Some(COURSE),
        1..=42,
        base(),
    ));
    let fold = log.fold();
    assert_eq!(fold.runs[0].set_code.as_deref(), Some("PRM"));
}

// Swiss Drafts: a draft played in a tournament, not a league

const SWISS: i64 = 19_990_050;
const SWISS_2: i64 = 19_990_058;

/// A draft-format tournament of the account whose first match starts at
/// `first_match_at`; its `EventSyncData_t` names no draft.
fn swiss(event_id: i64, format: &str, first_match_at: i64) -> DraftTournament {
    DraftTournament {
        event_id,
        format: Some(format.to_string()),
        first_match_at,
        draft_token: None,
        draft_id: None,
    }
}

/// A finished draft picking cards 2000..=2041 with no league, 25 minutes
/// long, MTGO draft id 7999556.
fn swiss_draft(log: &mut Log, token: &str, started_at: i64) {
    let mut found = draft(token, None, None, 2000..=2041, started_at);
    found.draft_id = Some(7_999_556);
    log.draft(found);
}

fn linked(fold: &LeagueFold) -> Vec<(&str, i64)> {
    fold.tournament_drafts
        .iter()
        .map(|link| (link.draft_token.as_str(), link.event_id))
        .collect()
}

#[test]
fn a_swiss_draft_links_to_the_draft_tournament_that_starts_after_it() {
    let mut log = Log::default();
    log.known(2000..=2041, Some("FRA"));
    swiss_draft(&mut log, DRAFT_TOKEN, base());
    // The draft ends at 12:25; deck building; round 1 at 12:37.
    log.facts
        .tournaments
        .push(swiss(SWISS, "DFRAFRAFRA", base() + 37 * MINUTE));
    let fold = log.fold();
    assert_eq!(linked(&fold), vec![(DRAFT_TOKEN, SWISS)]);
    assert!(fold.unlinked_drafts.is_empty());
    // A tournament is not a league: no run.
    assert!(fold.runs.is_empty());
}

#[test]
fn a_swiss_draft_links_by_the_draft_token_its_tournament_names() {
    let mut log = Log::default();
    swiss_draft(&mut log, DRAFT_TOKEN, base());
    // Named by its EventSyncData_t line: no time or set rule applies.
    let mut named = swiss(SWISS, "DFRAFRAFRA", base() + 5 * DAY);
    named.draft_token = Some(DRAFT_TOKEN.to_uppercase());
    log.facts.tournaments.push(named);
    log.facts
        .tournaments
        .push(swiss(SWISS_2, "DFRAFRAFRA", base() + 30 * MINUTE));
    assert_eq!(linked(&log.fold()), vec![(DRAFT_TOKEN, SWISS)]);
}

#[test]
fn a_swiss_draft_links_by_the_draft_id_its_tournament_names() {
    let mut log = Log::default();
    swiss_draft(&mut log, DRAFT_TOKEN, base());
    let mut named = swiss(SWISS, "DFRAFRAFRA", base() + 5 * DAY);
    named.draft_id = Some(7_999_556);
    log.facts.tournaments.push(named);
    assert_eq!(linked(&log.fold()), vec![(DRAFT_TOKEN, SWISS)]);
}

#[test]
fn a_draft_stays_unlinked_without_a_same_set_tournament_soon_after_it() {
    for (format, first_match_at) in [
        // Too late: past the window.
        (
            "DFRAFRAFRA",
            base() + 25 * MINUTE + TOURNAMENT_DRAFT_WINDOW_MS + MINUTE,
        ),
        // Before the draft ended.
        ("DFRAFRAFRA", base() + 20 * MINUTE),
        // Another set.
        ("DHOBHOBHOB", base() + 37 * MINUTE),
        // Sealed, not a draft.
        ("S6FRA", base() + 37 * MINUTE),
        // Constructed.
        ("CMODERN", base() + 37 * MINUTE),
    ] {
        let mut log = Log::default();
        log.known(2000..=2041, Some("FRA"));
        swiss_draft(&mut log, DRAFT_TOKEN, base());
        log.facts
            .tournaments
            .push(swiss(SWISS, format, first_match_at));
        let fold = log.fold();
        assert!(linked(&fold).is_empty(), "{format} at {first_match_at}");
        assert_eq!(fold.unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
    }
}

#[test]
fn each_swiss_draft_takes_the_nearest_tournament_once_in_any_order() {
    let mut log = Log::default();
    log.known(2000..=2041, Some("FRA"));
    swiss_draft(&mut log, DRAFT_TOKEN, base());
    swiss_draft(&mut log, DRAFT_TOKEN_2, base() + 45 * MINUTE);
    // Both tournaments start within an hour of the first draft's end; the
    // second is the second draft's.
    log.facts
        .tournaments
        .push(swiss(SWISS_2, "DFRAFRAFRA", base() + 82 * MINUTE));
    log.facts
        .tournaments
        .push(swiss(SWISS, "DFRAFRAFRA", base() + 37 * MINUTE));
    let fold = log.fold();
    let mut links = linked(&fold);
    links.sort();
    let mut expected = vec![(DRAFT_TOKEN, SWISS), (DRAFT_TOKEN_2, SWISS_2)];
    expected.sort();
    assert_eq!(links, expected);

    let mut reversed = log.clone();
    reversed.facts.drafts.reverse();
    reversed.facts.tournaments.reverse();
    let mut again = linked(&reversed.fold())
        .into_iter()
        .map(|(token, event)| (token.to_string(), event))
        .collect::<Vec<_>>();
    again.sort();
    let mut first = links
        .into_iter()
        .map(|(token, event)| (token.to_string(), event))
        .collect::<Vec<_>>();
    first.sort();
    assert_eq!(again, first);
}

#[test]
fn a_league_draft_never_links_to_a_tournament() {
    let mut log = Log::default();
    log.known(2000..=2041, Some("FRA"));
    log.joined(TOKEN, EVENT, base() - MINUTE);
    let mut league = draft(DRAFT_TOKEN, Some(EVENT), Some(COURSE), 2000..=2041, base());
    league.draft_id = Some(7_999_556);
    log.draft(league);
    let mut named = swiss(SWISS, "DFRAFRAFRA", base() + 37 * MINUTE);
    named.draft_id = Some(7_999_556);
    log.facts.tournaments.push(named);
    let fold = log.fold();
    assert!(linked(&fold).is_empty());
    assert_eq!(fold.runs.len(), 1);
    assert_eq!(fold.runs[0].draft_token.as_deref(), Some(DRAFT_TOKEN));
}

#[test]
fn a_league_draft_with_no_run_never_falls_back_to_a_tournament() {
    let mut log = Log::default();
    log.known(2000..=2041, Some("FRA"));
    // MTGO named a league the fold cannot place (an id past i64), so no run
    // holds the draft: it is still a league's draft, never a Swiss Draft.
    let mut league = draft(DRAFT_TOKEN, None, Some(COURSE), 2000..=2041, base());
    league.league_event_id = Some(u64::MAX);
    log.draft(league);
    log.facts
        .tournaments
        .push(swiss(SWISS, "DFRAFRAFRA", base() + 37 * MINUTE));
    let fold = log.fold();
    assert!(linked(&fold).is_empty());
    assert_eq!(fold.unlinked_drafts, vec![DRAFT_TOKEN.to_string()]);
}

#[test]
fn a_tournament_draft_state_comes_from_its_matches() {
    // (complete matches, swiss, last complete lost, newer activity).
    // Three rounds complete it, whatever came after.
    assert_eq!(
        tournament_draft_state(3, true, false, false),
        RunState::Complete
    );
    assert_eq!(
        tournament_draft_state(3, true, true, true),
        RunState::Complete
    );
    // A Swiss event plays on after a loss.
    assert_eq!(
        tournament_draft_state(1, true, true, false),
        RunState::Active
    );
    // An elimination event ends at the first loss.
    assert_eq!(
        tournament_draft_state(1, false, true, false),
        RunState::Complete
    );
    assert_eq!(
        tournament_draft_state(2, false, false, false),
        RunState::Active
    );
    assert_eq!(
        tournament_draft_state(0, false, false, false),
        RunState::Active
    );
    // A newer draft or a match outside the event drops it.
    assert_eq!(
        tournament_draft_state(2, true, false, true),
        RunState::Dropped
    );
    assert_eq!(
        tournament_draft_state(0, true, false, true),
        RunState::Dropped
    );
}

#[test]
fn a_tournament_draft_has_its_key_and_label() {
    assert_eq!(tournament_run_key(SWISS), "tournament:19990050");
    assert!(is_tournament_run_key("tournament:19990050"));
    assert!(!is_tournament_run_key("draft:19990050"));
    assert_eq!(
        tournament_draft_label(Some("Reality Fracture Swiss Draft")),
        "Swiss Draft"
    );
    assert_eq!(
        tournament_draft_label(Some("No Holds Barred Cube Elimination Draft")),
        "Tournament Draft"
    );
    assert_eq!(tournament_draft_label(None), "Tournament Draft");
}
