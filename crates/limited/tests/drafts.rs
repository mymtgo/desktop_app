//! The draft fold over the real (anonymised) draft fixtures and synthetic
//! lines. Ports 0.x `ProcessDraftEventsTest`, `ApplyDraftEventTest` and
//! `AbandonStaleDraftsTest`, plus live equals rebuild.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use fixture_tools::load_fixture;
use limited::{
    ABANDON_AFTER_MS, Draft, DraftEventInput, DraftFold, DraftState, Pick, fold_drafts, pool_counts,
};
use mtgo_protocol::{EventKind, FileContext, split_complete};
use regex::Regex;

const HOBBIT_TOKEN: &str = "791bacca-caea-4d88-b6c7-3bc067d412c2";
const INCOMPLETE_FIXTURE: &str = "draft_incomplete";

/// The fixtures were written in the UK: their payload offsets are +01:00.
const LONDON: Tz = chrono_tz::Europe::London;
const MINUTE_MS: i64 = 60 * 1000;

fn draft_kind(kind: EventKind) -> bool {
    matches!(
        kind,
        EventKind::DraftCreated
            | EventKind::DraftStateChanged
            | EventKind::DraftSelection
            | EventKind::DraftLeagueStanding
            | EventKind::DraftJoined
            | EventKind::DraftPodState
            | EventKind::DraftPackOpened
            | EventKind::DraftPendingPick
            | EventKind::DraftEnded
            | EventKind::DraftPickCommitted
            | EventKind::LeaguePoolGranted
    )
}

/// A fixture's draft events in log order, as the app reads them from `log_events`.
fn fixture_events(name: &str) -> Vec<DraftEventInput> {
    let fixture = load_fixture(name);
    let mut ctx = FileContext::new(1, fixture.meta.day_anchor(), LONDON, None);
    split_complete(&fixture.bytes)
        .iter()
        .filter_map(|raw| ctx.prepare(raw))
        .filter(|event| draft_kind(event.kind))
        .map(|event| DraftEventInput {
            kind: event.kind,
            draft_token: event.subjects.draft_token.clone(),
            logged_at_ms: event.at_ms,
            line_time: event.text.chars().take(8).collect(),
            raw: event.text,
        })
        .collect()
}

/// The fixture's `now` is days after the draft; the draft is finished anyway.
fn fixture_now(name: &str) -> i64 {
    load_fixture(name).meta.now_ms()
}

fn utc_ms(text: &str) -> i64 {
    DateTime::parse_from_rfc3339(text)
        .expect("RFC 3339")
        .timestamp_millis()
}

fn hobbit() -> Draft {
    let drafts = fold_drafts(
        &fixture_events("draft_hobbit"),
        LONDON,
        fixture_now("draft_hobbit"),
    );
    assert_eq!(drafts.len(), 1);
    drafts.into_iter().next().expect("one draft")
}

fn pick(draft: &Draft, ordinal: u32) -> &Pick {
    draft
        .picks
        .iter()
        .find(|pick| pick.ordinal == ordinal)
        .expect("pick exists")
}

// ---------------------------------------------------------------------------
// ProcessDraftEventsTest

#[test]
fn projects_the_hobbit_draft_with_42_picks() {
    let draft = hobbit();

    assert_eq!(draft.token, HOBBIT_TOKEN);
    assert_eq!(draft.league_event_id, Some(11039));
    assert_eq!(draft.course_id, Some(35746768));
    assert_eq!(draft.started_at_ms, Some(utc_ms("2026-08-22T11:00:12Z")));
    assert_eq!(draft.draft_id, Some(6781001));
    assert_eq!(draft.seat_count, 8);
    assert_eq!(draft.seat_index, Some(7));
    assert_eq!(draft.booster_catalog_id, Some(154732));
    assert_eq!(
        draft.pod_token.as_deref(),
        Some("8f89e066-390c-4438-a28f-c3570e4fdb55")
    );
    assert_eq!(draft.pack_size, 14);
    assert_eq!(draft.picks_expected, 42);
    assert_eq!(draft.state, DraftState::Finished);
    assert!(draft.ended_at_ms.is_some());
    assert_eq!(draft.picks.len(), 42);
    assert!(
        draft
            .picks
            .iter()
            .all(|pick| pick.picked_catalog_id.is_some())
    );
    // Every pick was committed by the server ack (0.x dispatched 42 commits).
    assert!(draft.picks.iter().all(|pick| pick.picked_card_id.is_some()));
    let ordinals: Vec<u32> = draft.picks.iter().map(|pick| pick.ordinal).collect();
    assert_eq!(ordinals, (1..=42).collect::<Vec<u32>>());
}

#[test]
fn records_pack_contents_pick_reservations_and_timing_for_p1p1() {
    let draft = hobbit();
    let p1p1 = pick(&draft, 1);

    assert_eq!(p1p1.pack_number, 1);
    assert_eq!(p1p1.pick_number, 1);
    assert_eq!(p1p1.pack_id, Some(143682097));
    assert_eq!(p1p1.direction, Some(0));
    assert_eq!(p1p1.cards_available.len(), 14);
    assert!(p1p1.cards_available.contains(&154228));
    assert!(p1p1.cards_available.contains(&154538));
    assert_eq!(p1p1.picked_catalog_id, Some(154228));
    assert_eq!(p1p1.picked_card_id, Some(2138582391));
    let reserved: Vec<u32> = p1p1.reservations.iter().map(|r| r.catalog_id).collect();
    assert_eq!(reserved, vec![154538, 154228]);
    let reserved_at: Vec<i64> = p1p1.reservations.iter().map(|r| r.at_ms).collect();
    assert_eq!(
        reserved_at,
        vec![
            utc_ms("2026-08-22T11:12:00Z"),
            utc_ms("2026-08-22T11:12:17Z")
        ]
    );
    // 0.x stores whole seconds: the payload's fractions are dropped.
    assert_eq!(p1p1.deadline_at_ms, Some(utc_ms("2026-08-22T11:13:09Z")));
    assert_eq!(p1p1.picked_at_ms, Some(utc_ms("2026-08-22T11:12:26Z")));
    // The line's local 12:12:00 on the deadline's date, in London.
    assert_eq!(p1p1.shown_at_ms, Some(utc_ms("2026-08-22T11:12:00Z")));
}

#[test]
fn derives_pack_and_pick_numbers_from_the_payload_so_the_wheel_lines_up() {
    let draft = hobbit();
    let p1p9 = pick(&draft, 9);
    let p2p1 = pick(&draft, 15);

    assert_eq!(p1p9.pack_id, Some(143682097));
    assert_eq!(p1p9.cards_available.len(), 6);
    assert_eq!(p1p9.picked_catalog_id, Some(153998));
    assert_eq!(p2p1.pack_number, 2);
    assert_eq!(p2p1.pick_number, 1);
    assert_eq!(p2p1.direction, Some(1));
}

#[test]
fn is_idempotent_across_refolds() {
    let events = fixture_events("draft_hobbit");
    let now = fixture_now("draft_hobbit");
    let first = fold_drafts(&events, LONDON, now);
    let second = fold_drafts(&events, LONDON, now);

    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].picks.len(), 42);
    assert_eq!(pick(&first[0], 1).reservations.len(), 2);
}

#[test]
fn projects_the_incomplete_fixture_as_a_finished_draft() {
    let drafts = fold_drafts(
        &fixture_events(INCOMPLETE_FIXTURE),
        LONDON,
        fixture_now(INCOMPLETE_FIXTURE),
    );

    assert_eq!(drafts.len(), 1);
    let draft = &drafts[0];
    assert_eq!(draft.league_event_id, Some(10814));
    assert_eq!(draft.course_id, Some(35460131));
    assert_eq!(draft.state, DraftState::Finished);
    assert_eq!(draft.picks.len(), 42);
    assert!(
        draft
            .picks
            .iter()
            .all(|pick| pick.picked_catalog_id.is_some())
    );
}

#[test]
fn fills_gaps_from_the_ended_message_when_pending_picks_were_missed() {
    let mut events = fixture_events("draft_hobbit");
    let mut dropped = 0;
    events.retain(|event| {
        if event.kind == EventKind::DraftPendingPick && dropped < 20 {
            dropped += 1;
            return false;
        }
        true
    });
    assert_eq!(dropped, 20);

    let drafts = fold_drafts(&events, LONDON, fixture_now("draft_hobbit"));
    let draft = &drafts[0];

    assert_eq!(draft.picks.len(), 42);
    assert_eq!(pick(draft, 1).picked_catalog_id, Some(154228));
    assert_eq!(pick(draft, 1).cards_available, vec![154228]);
    assert_eq!(pick(draft, 1).pack_number, 1);
    assert_eq!(pick(draft, 1).pick_number, 1);
    assert_eq!(pick(draft, 1).pack_id, Some(143682097));
    assert_eq!(pick(draft, 1).picked_card_id, Some(2138582391));
    assert_eq!(
        pick(draft, 1).picked_at_ms,
        Some(utc_ms("2026-08-22T11:12:26Z"))
    );
    assert_eq!(pick(draft, 1).shown_at_ms, None);
    assert_ne!(pick(draft, 21).cards_available, vec![154228]);
    assert_eq!(draft.state, DraftState::Finished);
}

#[test]
fn creates_the_draft_from_a_pending_pick_when_the_standing_was_missed() {
    let mut events = fixture_events("draft_hobbit");
    events.retain(|event| {
        !matches!(
            event.kind,
            EventKind::DraftLeagueStanding | EventKind::DraftJoined | EventKind::LeaguePoolGranted
        )
    });

    let drafts = fold_drafts(&events, LONDON, fixture_now("draft_hobbit"));

    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].league_event_id, None);
    assert_eq!(drafts[0].course_id, None);
    assert_eq!(drafts[0].seat_index, None);
    assert_eq!(drafts[0].picks.len(), 42);
}

#[test]
fn a_pending_pick_alone_creates_a_picking_draft() {
    let events: Vec<DraftEventInput> = fixture_events("draft_hobbit")
        .into_iter()
        .filter(|event| event.kind == EventKind::DraftPendingPick)
        .collect();
    let first_at = events[0].logged_at_ms;

    let drafts = fold_drafts(&events[..1], LONDON, first_at + MINUTE_MS);

    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].token, HOBBIT_TOKEN);
    assert_eq!(drafts[0].state, DraftState::Picking);
    assert_eq!(drafts[0].picks.len(), 1);
    assert_eq!(drafts[0].pack_size, 14);
    assert_eq!(drafts[0].picks_expected, 42);
    assert_eq!(drafts[0].draft_id, Some(6781001));
}

/// The fixture's `League User Initial Grant` rows, summed per catalog id.
fn granted_pool(name: &str) -> BTreeMap<u32, u32> {
    let row = Regex::new(r"CatalogID: (\d+), AddedQuantity: (\d+)").expect("valid regex");
    let grant = fixture_events(name)
        .into_iter()
        .find(|event| event.kind == EventKind::LeaguePoolGranted)
        .expect("pool grant");
    let mut pool = BTreeMap::new();
    for caps in row.captures_iter(&grant.raw) {
        let id: u32 = caps[1].parse().expect("id");
        let quantity: u32 = caps[2].parse().expect("quantity");
        *pool.entry(id).or_insert(0) += quantity;
    }
    pool
}

#[test]
fn pool_counts_match_the_league_pool_grant() {
    for name in ["draft_hobbit", INCOMPLETE_FIXTURE] {
        let drafts = fold_drafts(&fixture_events(name), LONDON, fixture_now(name));
        let counts = pool_counts(&drafts[0]);

        assert_eq!(counts.values().sum::<u32>(), 42, "{name}");
        assert_eq!(counts, granted_pool(name), "{name}");
    }
    assert_eq!(pool_counts(&hobbit()).get(&154228), Some(&2));
}

// ---------------------------------------------------------------------------
// Live equals rebuild

#[test]
fn folding_one_by_one_equals_folding_every_prefix_at_once() {
    for name in ["draft_hobbit", INCOMPLETE_FIXTURE] {
        let events = fixture_events(name);
        let mut live = DraftFold::new(LONDON);
        for (index, event) in events.iter().enumerate() {
            live.apply(event);
            let now = event.logged_at_ms + MINUTE_MS;
            let rebuilt = fold_drafts(&events[..=index], LONDON, now);
            assert_eq!(live.drafts(now), rebuilt, "{name}: after event {index}");
        }
    }
}

#[test]
fn committed_picks_never_change_as_more_events_arrive() {
    let events = fixture_events("draft_hobbit");
    let whole = hobbit();
    for end in 1..events.len() {
        let now = events[end - 1].logged_at_ms + MINUTE_MS;
        for draft in fold_drafts(&events[..end], LONDON, now) {
            assert_ne!(draft.state, DraftState::Abandoned, "prefix {end}");
            for partial in draft.picks.iter().filter(|p| p.picked_card_id.is_some()) {
                assert_eq!(partial, pick(&whole, partial.ordinal), "prefix {end}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Synthetic lines

const TOKEN_A: &str = "aaaaaaaa-0000-4000-8000-000000000001";
const TOKEN_B: &str = "bbbbbbbb-0000-4000-8000-000000000002";
const LOGIN_ID: u64 = 6031972;

/// 2026-08-22T12:00:00Z, the base of the synthetic timelines.
fn base_ms() -> i64 {
    Utc.with_ymd_and_hms(2026, 8, 22, 12, 0, 0)
        .single()
        .expect("valid")
        .timestamp_millis()
}

fn clock(at_ms: i64) -> String {
    DateTime::from_timestamp_millis(at_ms)
        .expect("valid")
        .format("%H:%M:%S")
        .to_string()
}

fn iso(at_ms: i64) -> String {
    DateTime::from_timestamp_millis(at_ms)
        .expect("valid")
        .format("%Y-%m-%dT%H:%M:%S%.3f+00:00")
        .to_string()
}

fn event(kind: EventKind, token: Option<&str>, at_ms: i64, body: &str) -> DraftEventInput {
    let raw = format!("{} [INF] {body}", clock(at_ms));
    DraftEventInput {
        kind,
        draft_token: token.map(str::to_string),
        logged_at_ms: at_ms,
        line_time: raw.chars().take(8).collect(),
        raw,
    }
}

fn created(token: &str, at_ms: i64) -> DraftEventInput {
    let body = format!("(Game Management|Draft Created: {token}) Details: Tournament Data:");
    event(EventKind::DraftCreated, Some(token), at_ms, &body)
}

fn standing(token: &str, league: u64, course: u64, start_ms: i64, at_ms: i64) -> DraftEventInput {
    let body = format!(
        "(Game Management|Processing Registered Handler for LeagueStanding_t in DraftUninitializedState) Message: {{\"LeagueID\":{league},\"LoginID\":{LOGIN_ID},\"CourseID\":{course},\"StartDate\":\"{}\",\"DraftGroupID\":6781001,\"DraftToken\":\"{token}\"}} Receiver: X",
        iso(start_ms)
    );
    event(EventKind::DraftLeagueStanding, Some(token), at_ms, &body)
}

/// A pending pick for pick `pick` (1-based) of pack 1 in a 14-card pack.
fn pending(token: &str, pick: u32, pack_id: u64, at_ms: i64) -> DraftEventInput {
    let count = 15 - pick;
    let cards: Vec<String> = (0..count).map(|i| (154200 + i).to_string()).collect();
    let body = format!(
        "(Game Management|Processing Registered Handler for FlsBoosterDraftPendingPickMessage in DraftWaitingState) Message: {{\"DraftToken\":\"{token}\",\"DraftID\":6781001,\"PendingPick\":{{\"BoosterInfo\":{{\"BoosterIndex\":0,\"CatalogID\":154732}},\"CardsAvailable\":[{}],\"EndTime\":\"{}\",\"Direction\":0,\"PackID\":{pack_id},\"CurrentSelections\":[],\"SelectionsPerPick\":1}} Receiver: X",
        cards.join(","),
        iso(at_ms + MINUTE_MS)
    );
    event(EventKind::DraftPendingPick, Some(token), at_ms, &body)
}

fn commit(
    token: &str,
    pack_id: u64,
    catalog_id: u32,
    card_id: u64,
    time_ms: i64,
    at_ms: i64,
) -> DraftEventInput {
    let body = format!(
        "(Game Management|Processing Registered Handler for FlsBoosterDraftPickSucceededMessage in DraftPickAwaitingValidationState) Message: {{\"DraftToken\":\"{token}\",\"DraftID\":6781001,\"Selections\":[{{\"CatalogID\":{catalog_id},\"IsReservation\":false}}],\"PicksMade\":[{{\"LoginID\":{LOGIN_ID},\"PackID\":{pack_id},\"Selections\":[{{\"CardID\":{card_id},\"CatalogID\":{catalog_id}}}],\"Time\":\"{}\"}}],\"IsPickComplete\":true}} Receiver: X",
        iso(time_ms)
    );
    event(EventKind::DraftPickCommitted, Some(token), at_ms, &body)
}

fn selection(reserved: &str, committed: &str, at_ms: i64) -> DraftEventInput {
    let body = format!(
        "(Game Management|Processing Registered Handler for SubmitDraftSelectionsAction in DraftPickingState) Message: SubmitDraftSelectionsAction: Reserved - {reserved} ~ Committed - {committed} Receiver: X"
    );
    event(EventKind::DraftSelection, None, at_ms, &body)
}

fn finished(token: &str, at_ms: i64) -> DraftEventInput {
    let body = format!(
        "(Game Management|Draft State Changed for {token} from DraftWaitingState to DraftFinishedState)"
    );
    event(EventKind::DraftStateChanged, Some(token), at_ms, &body)
}

fn pool_granted(league: u64, rows: &[(u32, u32)], at_ms: i64) -> DraftEventInput {
    let mut body = format!(
        "(Collection Management|League User Initial Grant) Adding limited league pool to collection for league: {league}, unique catalog ids: {}",
        rows.len()
    );
    for (catalog_id, quantity) in rows {
        body.push_str(&format!(
            "\nCatalogID: {catalog_id}, AddedQuantity: {quantity}, OldQuantity: 0, NewQuantity: {quantity}"
        ));
    }
    event(EventKind::LeaguePoolGranted, None, at_ms, &body)
}

fn draft<'a>(drafts: &'a [Draft], token: &str) -> &'a Draft {
    drafts
        .iter()
        .find(|draft| draft.token == token)
        .expect("draft exists")
}

// ApplyDraftEventTest

#[test]
fn does_not_stamp_a_replayed_commit_onto_an_unrelated_pending_pick() {
    let base = base_ms();
    let mut events = Vec::new();
    // Picks 1 to 8 are committed; pick 9's pack is still on the table.
    for ordinal in 1..=9u32 {
        let at = base + i64::from(ordinal) * MINUTE_MS;
        let pack_id = 143682000 + u64::from(ordinal);
        events.push(pending(TOKEN_A, ordinal, pack_id, at));
        if ordinal < 9 {
            let catalog_id = 154220 + ordinal;
            let card_id = 2138582380 + u64::from(ordinal);
            events.push(commit(
                TOKEN_A,
                pack_id,
                catalog_id,
                card_id,
                at + 1000,
                at + 2000,
            ));
        }
    }
    let replay_at = base + 10 * MINUTE_MS;
    events.push(commit(
        TOKEN_A, 143682005, 154225, 2138582385, base, replay_at,
    ));

    let drafts = fold_drafts(&events, chrono_tz::UTC, replay_at + MINUTE_MS);
    let draft = draft(&drafts, TOKEN_A);
    let nine = pick(draft, 9);
    let five = pick(draft, 5);

    assert_eq!(nine.picked_catalog_id, None);
    assert_eq!(nine.picked_card_id, None);
    assert_eq!(nine.picked_at_ms, None);
    assert_eq!(five.picked_catalog_id, Some(154225));
    assert_eq!(five.picked_card_id, Some(2138582385));
    assert_eq!(five.picked_at_ms, Some(base + 5 * MINUTE_MS + 1000));
}

#[test]
fn still_applies_a_first_time_commit_whose_card_the_selection_line_already_recorded() {
    let base = base_ms();
    let events = vec![
        pending(TOKEN_A, 1, 143682001, base),
        pending(TOKEN_A, 2, 143682002, base + MINUTE_MS),
        // SubmitDraftSelectionsAction lands first and writes the catalog id only.
        selection("", "154228", base + 2 * MINUTE_MS),
        pending(TOKEN_A, 3, 143682003, base + 3 * MINUTE_MS),
        commit(
            TOKEN_A,
            143682002,
            154228,
            2138582391,
            utc_ms("2026-08-22T11:12:26.315Z"),
            base + 4 * MINUTE_MS,
        ),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + 5 * MINUTE_MS);
    let two = pick(draft(&drafts, TOKEN_A), 2);

    assert_eq!(two.picked_catalog_id, Some(154228));
    assert_eq!(two.picked_card_id, Some(2138582391));
    assert_eq!(two.picked_at_ms, Some(utc_ms("2026-08-22T11:12:26Z")));
}

// AbandonStaleDraftsTest

#[test]
fn abandons_a_picking_draft_with_no_pick_activity_for_30_minutes() {
    let now = base_ms();
    let token_c = "cccccccc-0000-4000-8000-000000000003";
    let events = vec![
        pending(token_c, 1, 1, now - 60 * MINUTE_MS),
        finished(token_c, now - 59 * MINUTE_MS),
        pending(TOKEN_A, 1, 2, now - 45 * MINUTE_MS),
        pending(TOKEN_B, 1, 3, now - 5 * MINUTE_MS),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, now);

    assert_eq!(draft(&drafts, TOKEN_A).state, DraftState::Abandoned);
    assert_eq!(draft(&drafts, TOKEN_B).state, DraftState::Picking);
    assert_eq!(draft(&drafts, token_c).state, DraftState::Finished);
}

#[test]
fn uses_the_draft_start_when_no_pick_was_ever_shown() {
    let now = base_ms();
    let events = vec![standing(
        TOKEN_A,
        11039,
        1,
        now - 60 * MINUTE_MS,
        now - MINUTE_MS,
    )];

    let drafts = fold_drafts(&events, chrono_tz::UTC, now);

    assert_eq!(drafts[0].state, DraftState::Abandoned);
}

#[test]
fn counts_a_recent_pick_as_activity_even_when_the_pack_was_shown_long_ago() {
    let now = base_ms();
    let events = vec![
        standing(
            TOKEN_A,
            11039,
            1,
            now - 120 * MINUTE_MS,
            now - 46 * MINUTE_MS,
        ),
        pending(TOKEN_A, 1, 143682001, now - 45 * MINUTE_MS),
        commit(
            TOKEN_A,
            143682001,
            154200,
            2138582391,
            now - 2 * MINUTE_MS,
            now - 2 * MINUTE_MS,
        ),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, now);

    assert_eq!(drafts[0].state, DraftState::Picking);
}

#[test]
fn falls_back_to_the_first_event_for_a_connecting_draft_with_no_picks_and_no_start() {
    let now = base_ms();
    let drafts = fold_drafts(
        &[created(TOKEN_A, now - 120 * MINUTE_MS)],
        chrono_tz::UTC,
        now,
    );
    assert_eq!(drafts[0].state, DraftState::Abandoned);

    let fresh = fold_drafts(
        &[created(TOKEN_A, now - 29 * MINUTE_MS)],
        chrono_tz::UTC,
        now,
    );
    assert_eq!(fresh[0].state, DraftState::Connecting);
}

#[test]
fn abandonment_needs_strictly_more_than_30_minutes() {
    let now = base_ms();
    let at_limit = fold_drafts(
        &[created(TOKEN_A, now - ABANDON_AFTER_MS)],
        chrono_tz::UTC,
        now,
    );
    assert_eq!(at_limit[0].state, DraftState::Connecting);
}

// Token-less lines

#[test]
fn a_selection_goes_to_the_picking_draft_with_the_latest_shown_pick() {
    let base = base_ms();
    let events = vec![
        pending(TOKEN_A, 1, 1, base),
        pending(TOKEN_B, 1, 2, base + MINUTE_MS),
        selection("154201", "", base + 2 * MINUTE_MS),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + 3 * MINUTE_MS);

    assert!(pick(draft(&drafts, TOKEN_A), 1).reservations.is_empty());
    let b = pick(draft(&drafts, TOKEN_B), 1);
    assert_eq!(b.reservations.len(), 1);
    assert_eq!(b.reservations[0].catalog_id, 154201);
    assert_eq!(b.reservations[0].at_ms, base + 2 * MINUTE_MS);
}

#[test]
fn a_selection_skips_a_draft_that_went_quiet_and_finished_drafts() {
    let base = base_ms();
    let events = vec![
        // Draft A stopped after its first pack: abandoned by the time the line lands.
        pending(TOKEN_A, 1, 1, base),
        // Draft B finished.
        pending(TOKEN_B, 1, 2, base + 40 * MINUTE_MS),
        finished(TOKEN_B, base + 41 * MINUTE_MS),
        selection("", "154201", base + 42 * MINUTE_MS),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + 43 * MINUTE_MS);

    assert_eq!(pick(draft(&drafts, TOKEN_A), 1).picked_catalog_id, None);
    assert_eq!(pick(draft(&drafts, TOKEN_B), 1).picked_catalog_id, None);
    assert_eq!(draft(&drafts, TOKEN_A).state, DraftState::Abandoned);
}

#[test]
fn a_selection_commit_targets_the_highest_unpicked_pick_and_dedupes_reservations() {
    let base = base_ms();
    let events = vec![
        pending(TOKEN_A, 1, 1, base),
        selection("154201,154202", "", base + 1000),
        selection("154201", "", base + 1000),
        selection("", "154203", base + 2000),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + MINUTE_MS);
    let one = pick(&drafts[0], 1);

    assert_eq!(one.reservations.len(), 1);
    assert_eq!(one.reservations[0].catalog_id, 154201);
    assert_eq!(one.picked_catalog_id, Some(154203));
    assert_eq!(one.picked_at_ms, Some(base + 2000));
    assert_eq!(one.picked_card_id, None);
}

#[test]
fn a_pool_grant_links_the_active_draft_when_no_draft_has_that_league() {
    let base = base_ms();
    let events = vec![
        pending(TOKEN_A, 1, 1, base),
        pool_granted(11039, &[(154200, 1)], base + MINUTE_MS),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + 2 * MINUTE_MS);

    assert_eq!(drafts[0].league_event_id, Some(11039));
    assert_eq!(drafts[0].course_id, None);
}

#[test]
fn a_pool_grant_prefers_the_draft_already_in_that_league() {
    let base = base_ms();
    let events = vec![
        standing(TOKEN_A, 11039, 7, base - MINUTE_MS, base),
        pending(TOKEN_A, 1, 1, base),
        finished(TOKEN_A, base + MINUTE_MS),
        pending(TOKEN_B, 1, 2, base + 2 * MINUTE_MS),
        pool_granted(11039, &[(154200, 1)], base + 3 * MINUTE_MS),
    ];

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + 4 * MINUTE_MS);

    assert_eq!(draft(&drafts, TOKEN_A).league_event_id, Some(11039));
    assert_eq!(draft(&drafts, TOKEN_B).league_event_id, None);
}

// Timestamps

#[test]
fn shown_at_is_the_line_time_on_the_deadline_date_in_the_local_timezone() {
    // Deadline 00:00:30 BST on 23 August; the line was written at 23:59:50 BST
    // the evening before. The line time is far after the deadline's date, so it
    // belongs to the day before (0.x `ConvertMtgoTimestamp`).
    let body = format!(
        "(Game Management|Processing Registered Handler for FlsBoosterDraftPendingPickMessage in DraftWaitingState) Message: {{\"DraftToken\":\"{TOKEN_A}\",\"PendingPick\":{{\"BoosterInfo\":{{\"BoosterIndex\":1}},\"CardsAvailable\":[1,2,3],\"EndTime\":\"2026-08-23T00:00:30.5000000+01:00\",\"Direction\":1,\"PackID\":9}}"
    );
    let raw = format!("23:59:50 [INF] {body}");
    let logged_at = utc_ms("2026-08-22T22:59:50Z");
    let input = DraftEventInput {
        kind: EventKind::DraftPendingPick,
        draft_token: Some(TOKEN_A.to_string()),
        logged_at_ms: logged_at,
        line_time: "23:59:50".to_string(),
        raw,
    };

    let drafts = fold_drafts(&[input], LONDON, logged_at);
    let only = &drafts[0].picks[0];

    assert_eq!(only.shown_at_ms, Some(logged_at));
    assert_eq!(only.deadline_at_ms, Some(utc_ms("2026-08-22T23:00:30Z")));
    // Pack 2 (BoosterIndex 1) of a 14-card default: pick 12, ordinal 26.
    assert_eq!(only.pack_number, 2);
    assert_eq!(only.pick_number, 12);
    assert_eq!(only.ordinal, 26);
    assert_eq!(only.direction, Some(1));
}

#[test]
fn shown_at_falls_back_to_the_logged_time_without_a_deadline() {
    let base = base_ms();
    let body = format!(
        "(Game Management|Processing Registered Handler for FlsBoosterDraftPendingPickMessage in DraftWaitingState) Message: {{\"DraftToken\":\"{TOKEN_A}\",\"PendingPick\":{{\"BoosterInfo\":{{\"BoosterIndex\":0}},\"CardsAvailable\":[1,2]}}"
    );
    let input = event(EventKind::DraftPendingPick, Some(TOKEN_A), base, &body);

    let drafts = fold_drafts(&[input], LONDON, base);

    assert_eq!(drafts[0].picks[0].shown_at_ms, Some(base));
    assert_eq!(drafts[0].picks[0].deadline_at_ms, None);
}

// Hostile input

fn hostile(kind: EventKind, body: &str, at_ms: i64) -> DraftEventInput {
    event(kind, Some(HOBBIT_TOKEN), at_ms, body)
}

fn hostile_pending(booster_index: &str, cards: &str, at_ms: i64) -> DraftEventInput {
    let body = format!(
        "(Game Management|Processing Registered Handler for FlsBoosterDraftPendingPickMessage in DraftWaitingState) Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"DraftID\":6781001,\"PendingPick\":{{\"BoosterInfo\":{{\"BoosterIndex\":{booster_index}}},\"CardsAvailable\":{cards},\"PackID\":1,\"Direction\":0}} Receiver: X"
    );
    hostile(EventKind::DraftPendingPick, &body, at_ms)
}

#[test]
fn hostile_payloads_never_panic_or_touch_existing_picks() {
    let mut events = fixture_events("draft_hobbit");
    let now = fixture_now("draft_hobbit");
    let before = hobbit();
    let at = events.last().expect("events").logged_at_ms;
    let fourteen = format!("[{}]", vec!["1"; 14].join(","));

    for booster_index in [
        "4294967295",
        "400000000",
        // 2^31 * 14 wraps to 0 in u32: unchecked, this lands on ordinal 1.
        "2147483648",
        "18446744073709551615",
        "-1",
        "1.5",
        "\"abc\"",
        "[]",
    ] {
        events.push(hostile_pending(booster_index, &fourteen, at));
    }
    events.push(hostile_pending("0", "\"x\"", at));
    // Junk card ids read as 0, as PHP `intval` did; on its own draft so the
    // well-formed numbering (pack 1, pick 11) cannot land on a hobbit pick.
    let mut junk_cards = hostile_pending("0", "[-1,\"a\",null,99999999999]", at);
    junk_cards.raw = junk_cards.raw.replace(HOBBIT_TOKEN, TOKEN_B);
    junk_cards.draft_token = Some(TOKEN_B.to_string());
    events.push(junk_cards);
    events.push(hostile(
        EventKind::DraftPickCommitted,
        &format!("Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"PicksMade\":\"x\"}}"),
        at,
    ));
    events.push(hostile(
        EventKind::DraftPickCommitted,
        &format!("Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"PicksMade\":[1,{{\"PackID\":-5,\"Selections\":\"x\"}}]}}"),
        at,
    ));
    events.push(hostile(
        EventKind::DraftEnded,
        &format!("Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"Picks\":\"x\"}}"),
        at,
    ));
    events.push(hostile(
        EventKind::DraftEnded,
        &format!("Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"Picks\":[1,\"a\",null,{{\"Selections\":[{{\"CatalogID\":-1,\"CardID\":\"z\"}}],\"Time\":\"never\"}}]}}"),
        at,
    ));
    events.push(hostile(
        EventKind::DraftLeagueStanding,
        &format!("Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"LeagueID\":-1,\"LoginID\":\"q\",\"StartDate\":7}}"),
        at,
    ));
    events.push(hostile(
        EventKind::DraftPodState,
        &format!("Message: {{\"DraftToken\":\"{HOBBIT_TOKEN}\",\"Pods\":\"x\"}}"),
        at,
    ));
    events.push(hostile(EventKind::DraftPendingPick, "Message: {{{{[[[", at));
    events.push(selection("99999999999999", "-1", at));
    events.push(pool_granted(u64::MAX, &[(u32::MAX, u32::MAX)], at));

    let drafts = fold_drafts(&events, LONDON, now);
    let after = draft(&drafts, HOBBIT_TOKEN);

    for original in &before.picks {
        assert_eq!(pick(after, original.ordinal), original);
    }
    assert_eq!(after.pack_size, 14);
    assert_eq!(after.picks_expected, 42);
    assert_eq!(draft(&drafts, TOKEN_B).picks[0].cards_available, vec![0; 4]);
}

#[test]
fn a_hostile_pending_pick_mid_draft_is_ignored() {
    let base = base_ms();
    let fourteen = format!("[{}]", vec!["1"; 14].join(","));
    let mut events = vec![pending(HOBBIT_TOKEN, 1, 143682001, base)];
    for booster_index in ["2147483648", "-1", "4294967295"] {
        events.push(hostile_pending(booster_index, &fourteen, base + 1000));
    }

    let drafts = fold_drafts(&events, chrono_tz::UTC, base + MINUTE_MS);

    assert_eq!(drafts[0].picks.len(), 1);
    assert_eq!(drafts[0].picks[0].pack_id, Some(143682001));
    assert_eq!(drafts[0].picks[0].cards_available.len(), 14);
    assert_ne!(drafts[0].picks[0].cards_available, vec![1; 14]);
}

// Order independence (ruling 3)

#[test]
fn the_seat_is_found_when_the_standing_comes_after_the_pod_state() {
    let mut events = fixture_events("draft_hobbit");
    let at = events
        .iter()
        .position(|event| event.kind == EventKind::DraftLeagueStanding)
        .expect("standing");
    let standing = events.remove(at);
    events.push(standing);

    let drafts = fold_drafts(&events, LONDON, fixture_now("draft_hobbit"));

    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].seat_index, Some(7));
    assert_eq!(drafts[0], hobbit());
}
