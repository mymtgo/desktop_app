//! Ports 0.x `ComputeSeenWheelTest` and `ComputeDraftSignalsTest`.

mod common;

use std::collections::BTreeMap;

use common::{card, facts, pack_pick};
use limited::{ColorSignal, SeenFacts, WheelView, draft_signals, seen_wheel, wheel_for_pick};

#[test]
fn derives_seen_wheel_and_passed_for_an_8_seat_pod() {
    let picks = vec![
        pack_pick(1, 500, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14], 8),
        pack_pick(
            2,
            501,
            &[20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32],
            20,
        ),
        pack_pick(
            3,
            502,
            &[40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51],
            40,
        ),
        pack_pick(4, 503, &[60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70], 60),
        pack_pick(5, 504, &[80, 81, 82, 83, 84, 85, 86, 87, 88, 89], 80),
        pack_pick(6, 505, &[90, 91, 92, 93, 94, 95, 96, 97, 98], 90),
        pack_pick(7, 506, &[100, 101, 102, 103, 104, 105, 106, 107], 100),
        pack_pick(8, 507, &[110, 111, 112, 113, 114, 115, 116], 110),
        pack_pick(9, 500, &[1, 2, 5, 6, 11, 14], 1),
        pack_pick(10, 501, &[21, 22, 23, 24, 25], 21),
    ];

    let result = seen_wheel(&picks, 8);

    assert_eq!(
        result[&1],
        SeenFacts {
            seen_count: 2,
            first_seen_ordinal: 1,
            wheeled: true,
            wheeled_to_me: true,
            picked_count: 1,
            passed_count: 1,
        }
    );
    let two = &result[&2];
    assert_eq!(
        (
            two.seen_count,
            two.wheeled,
            two.wheeled_to_me,
            two.picked_count,
            two.passed_count
        ),
        (2, true, false, 0, 2)
    );
    let three = &result[&3];
    assert_eq!(
        (three.seen_count, three.wheeled, three.passed_count),
        (1, false, 1)
    );
    let eight = &result[&8];
    assert_eq!(
        (
            eight.seen_count,
            eight.wheeled,
            eight.picked_count,
            eight.passed_count
        ),
        (1, false, 1, 0)
    );
    let twenty_one = &result[&21];
    assert_eq!(
        (
            twenty_one.seen_count,
            twenty_one.wheeled,
            twenty_one.wheeled_to_me
        ),
        (2, true, true)
    );
}

#[test]
fn uses_seat_count_for_the_wheel_distance_in_a_6_seat_pod() {
    let picks = vec![
        pack_pick(1, 900, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14], 1),
        pack_pick(2, 901, &[20, 21, 22], 20),
        pack_pick(3, 902, &[30, 31], 30),
        pack_pick(4, 903, &[40], 40),
        pack_pick(5, 904, &[50], 50),
        pack_pick(6, 905, &[60], 60),
        pack_pick(7, 900, &[2, 3, 4, 5, 6, 7, 8, 9], 2),
    ];

    let result = seen_wheel(&picks, 6);

    assert!(result[&2].wheeled);
    assert!(result[&2].wheeled_to_me);
    assert!(!result[&14].wheeled);
}

#[test]
fn does_not_treat_a_different_pack_at_the_wheel_distance_as_a_wheel() {
    let picks = vec![
        pack_pick(1, 500, &[1, 2, 3], 1),
        pack_pick(9, 999, &[2, 3], 2),
    ];

    assert!(!seen_wheel(&picks, 8)[&2].wheeled);
}

#[test]
fn describes_the_wheel_view_for_a_pick() {
    let picks = vec![
        pack_pick(1, 500, &[1, 2, 3, 4], 4),
        pack_pick(9, 500, &[1, 3], 1),
    ];

    assert_eq!(
        wheel_for_pick(&picks, &picks[0], 8),
        Some(WheelView {
            return_ordinal: 9,
            survived: vec![1, 3],
            taken: vec![2],
        })
    );
    assert_eq!(wheel_for_pick(&picks, &picks[1], 8), None);
}

#[test]
fn ranks_colours_by_wheeled_and_seen_twice_counts() {
    let seen = |seen_count: u32, wheeled: bool| SeenFacts {
        seen_count,
        first_seen_ordinal: 1,
        wheeled,
        wheeled_to_me: false,
        picked_count: 0,
        passed_count: seen_count,
    };
    let seen_wheel: BTreeMap<u32, SeenFacts> = BTreeMap::from([
        (1, seen(2, true)),
        (2, seen(2, true)),
        (3, seen(3, false)),
        (4, seen(1, false)),
        (5, seen(2, true)),
    ]);
    let cards = facts(vec![
        (1, card("Creature", "U")),
        (2, card("Creature", "U")),
        (3, card("Creature", "UG")),
        (4, card("Creature", "R")),
        (5, card("Artifact", "")),
    ]);

    let signals = draft_signals(&seen_wheel, &cards);

    assert_eq!(signals.len(), 5);
    assert_eq!(
        signals[0],
        ColorSignal {
            color: 'U',
            wheeled: 2,
            seen_twice: 3,
            score: 5,
            share: 1.0,
        }
    );
    assert_eq!(
        (
            signals[1].color,
            signals[1].wheeled,
            signals[1].seen_twice,
            signals[1].score
        ),
        ('G', 0, 1, 1)
    );
    assert_eq!(signals[1].share, 0.2);
    let red = signals.iter().find(|signal| signal.color == 'R').unwrap();
    assert_eq!(
        (red.wheeled, red.seen_twice, red.score, red.share),
        (0, 0, 0, 0.0)
    );
    // Ties keep WUBRG order.
    let order: Vec<char> = signals.iter().map(|signal| signal.color).collect();
    assert_eq!(order, vec!['U', 'G', 'W', 'B', 'R']);
}

#[test]
fn unknown_cards_add_no_colour_signal() {
    let seen_wheel = BTreeMap::from([(
        7,
        SeenFacts {
            seen_count: 3,
            first_seen_ordinal: 1,
            wheeled: true,
            wheeled_to_me: false,
            picked_count: 0,
            passed_count: 3,
        },
    )]);

    let signals = draft_signals(&seen_wheel, &facts(vec![]));

    assert!(signals.iter().all(|signal| signal.score == 0));
    assert!(signals.iter().all(|signal| signal.share == 0.0));
}

#[test]
fn an_absurd_seat_count_does_not_overflow_the_wheel_distance() {
    let picks = vec![
        pack_pick(1, 500, &[1, 2, 3], 1),
        pack_pick(2, 501, &[4, 5], 4),
    ];

    let result = seen_wheel(&picks, u32::MAX);

    assert!(!result[&2].wheeled);
    assert_eq!(wheel_for_pick(&picks, &picks[1], u32::MAX), None);
}
