//! Ports the pure cases of 0.x `BuildLimitedIndexTest`: the per-draft pick
//! time and the KPI strip.

mod common;

use common::{ELEVEN_AM, SECOND_MS, draft, pick};
use limited::{
    DraftPickStats, IndexRun, LimitedIndexKpis, Pick, Reservation, draft_pick_stats,
    limited_index_kpis,
};

/// 0.x `draftWithPicks`: 42 picks, each committed 12 seconds after it was
/// shown, every fifth one with two reservations.
fn draft_with_picks() -> limited::Draft {
    let picks = (1..=42)
        .map(|ordinal: u32| {
            let shown = ELEVEN_AM + i64::from(ordinal) * 30 * SECOND_MS;
            let reservations = if ordinal.is_multiple_of(5) {
                vec![
                    Reservation {
                        catalog_id: 1,
                        at_ms: shown,
                    },
                    Reservation {
                        catalog_id: 2,
                        at_ms: shown,
                    },
                ]
            } else {
                Vec::new()
            };
            Pick {
                pack_number: (ordinal - 1) / 14 + 1,
                pick_number: (ordinal - 1) % 14 + 1,
                picked_catalog_id: Some(1000 + ordinal),
                shown_at_ms: Some(shown),
                picked_at_ms: Some(shown + 12 * SECOND_MS),
                reservations,
                ..pick(ordinal)
            }
        })
        .collect();
    draft("q-draft", picks)
}

fn run(
    set_code: Option<&str>,
    complete: bool,
    wins: u32,
    losses: u32,
    has_draft: bool,
) -> IndexRun {
    IndexRun {
        set_code: set_code.map(str::to_string),
        complete,
        wins,
        losses,
        matches: wins + losses,
        has_draft,
    }
}

#[test]
fn a_draft_row_shows_its_average_pick_time() {
    let stats = draft_pick_stats(&draft_with_picks()).unwrap();

    assert_eq!(stats.avg_seconds, Some(12));
    assert_eq!((stats.indecisive, stats.picks), (8, 42));
}

#[test]
fn counts_unlinked_drafts_as_events_and_drafts() {
    let kpis = limited_index_kpis(&[], 1, &[]);

    assert_eq!((kpis.events, kpis.drafts, kpis.unlinked), (1, 1, 1));
    assert_eq!(kpis.match_win_pct, None);
    assert_eq!(kpis.avg_pick_seconds, None);
    assert_eq!(kpis.indecision_pct, None);
}

#[test]
fn computes_kpis_over_the_filtered_runs() {
    let stats: Vec<DraftPickStats> = draft_pick_stats(&draft_with_picks()).into_iter().collect();
    let runs = [
        run(Some("HOB"), true, 1, 0, true),
        run(Some("MSH"), true, 0, 1, false),
    ];

    let kpis = limited_index_kpis(&runs, 0, &stats);

    assert_eq!(
        kpis,
        LimitedIndexKpis {
            events: 2,
            drafts: 1,
            unlinked: 0,
            match_win_pct: Some(50),
            match_wins: 1,
            match_losses: 1,
            avg_wins: Some(0.5),
            avg_losses: Some(0.5),
            completed_runs: 2,
            most_drafted_set: Some("HOB".to_string()),
            most_drafted_count: 1,
            avg_pick_seconds: Some(12),
            indecision_pct: Some(19),
        }
    );
}

#[test]
fn averages_only_complete_runs_with_matches() {
    let runs = [
        run(Some("HOB"), true, 3, 0, true),
        run(Some("HOB"), true, 2, 1, true),
        run(Some("HOB"), true, 0, 0, true),
        run(Some("LTR"), false, 1, 1, true),
        run(None, false, 0, 0, false),
    ];

    let kpis = limited_index_kpis(&runs, 0, &[]);

    assert_eq!(kpis.completed_runs, 2);
    assert_eq!((kpis.avg_wins, kpis.avg_losses), (Some(2.5), Some(0.5)));
    assert_eq!(kpis.most_drafted_set.as_deref(), Some("HOB"));
    assert_eq!(kpis.most_drafted_count, 3);
    assert_eq!(kpis.match_win_pct, Some(75));
}

#[test]
fn the_average_pick_time_is_the_mean_of_per_draft_averages() {
    let stats = [
        DraftPickStats {
            avg_seconds: Some(10),
            indecisive: 1,
            picks: 10,
        },
        DraftPickStats {
            avg_seconds: Some(15),
            indecisive: 0,
            picks: 30,
        },
        DraftPickStats {
            avg_seconds: None,
            indecisive: 0,
            picks: 2,
        },
    ];

    let kpis = limited_index_kpis(&[], 0, &stats);

    // (10 + 15) / 2 = 12.5 rounds half up; 1 of 42 picks is 2%.
    assert_eq!(kpis.avg_pick_seconds, Some(13));
    assert_eq!(kpis.indecision_pct, Some(2));
}
