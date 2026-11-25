//! Fake players for the builder's tests (fake names only). Each builder gives
//! a `WrappedInput` shaped like one of the UI's fake bundles: `light` (just
//! over the floor), `grinder` (every card) and `limited` (drafts only).

#![allow(dead_code)] // a toolbox: each test uses only some helpers

use chrono::TimeZone;
use chrono_tz::Tz;

use crate::bundle::{ArchetypeLook, Art, KeyCard};
use crate::input::*;
use crate::window::year_window;

const MINUTE_MS: i64 = 60_000;

/// A local wall-clock time as UTC milliseconds.
pub fn local_ms(tz: Tz, year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    tz.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .expect("an unambiguous local time")
        .timestamp_millis()
}

/// An empty input for a year: no matches, nothing else. Tests fill it in.
pub fn empty(tz: Tz) -> WrappedInput {
    WrappedInput {
        year: 2026,
        window: year_window(2026, tz),
        built_at_ms: local_ms(tz, 2026, 12, 2, 9, 0),
        tz,
        first_year: true,
        matches: Vec::new(),
        decks: Vec::new(),
        archetypes: Vec::new(),
        cards: Vec::new(),
        deck_games: Vec::new(),
        runs: Vec::new(),
        finishes: Vec::new(),
        drafts: Vec::new(),
    }
}

pub fn art(name: &str) -> Art {
    Art {
        remote: Some(format!("https://cards.example.test/art/{name}.jpg")),
        local: None,
        custom: None,
    }
}

pub fn image(name: &str) -> Art {
    Art {
        remote: Some(format!("https://cards.example.test/full/{name}.jpg")),
        local: None,
        custom: None,
    }
}

pub fn key_card(name: &str) -> KeyCard {
    KeyCard {
        name: name.to_string(),
        image: image(name),
    }
}

pub fn deck(id: i64, name: &str, format: &str, colors: &[&str], cards: &[&str]) -> InputDeck {
    InputDeck {
        id,
        name: name.to_string(),
        format_label: format.to_string(),
        colors: colors.iter().map(|c| c.to_string()).collect(),
        art: art(name),
        key_cards: cards.iter().map(|c| key_card(c)).collect(),
    }
}

pub fn archetype(id: i64, name: &str, colors: &[&str], cards: &[&str]) -> InputArchetype {
    InputArchetype {
        id,
        name: name.to_string(),
        look: ArchetypeLook {
            art: art(name),
            colors: colors.iter().map(|c| c.to_string()).collect(),
            cards: cards.iter().map(|c| key_card(c)).collect(),
        },
    }
}

/// A card row for a deck; the numbers are the caller's.
pub fn card(
    deck_id: i64,
    name: &str,
    is_land: bool,
    cast: (u32, u32),
    hands: u32,
    sided: u32,
) -> InputCard {
    InputCard {
        deck_id,
        name: name.to_string(),
        is_land,
        cast_games: cast.0,
        cast_won: cast.1,
        total_cast: cast.0 + cast.0 / 2,
        opening_hand_games: hands,
        sided_in_games: sided,
        art: art(name),
        image: image(name),
    }
}

/// A match with no game rows (an imported or history match).
pub fn plain_match(
    started_at_ms: i64,
    format: &str,
    deck_id: Option<i64>,
    result: MatchResult,
) -> InputMatch {
    InputMatch {
        started_at_ms,
        ended_at_ms: Some(started_at_ms + 40 * MINUTE_MS),
        format_label: format.to_string(),
        limited: format == "Limited",
        deck_id,
        result,
        opponent_archetype_id: None,
        game_count: if result == MatchResult::Win { 2 } else { 3 },
        games: Vec::new(),
    }
}

/// A match with game rows: two won games for a win, two lost for a loss, a
/// one-one draw otherwise. `seed` varies who is on the play, mulligans and
/// the turn a game ended.
pub fn game_match(
    started_at_ms: i64,
    format: &str,
    deck_id: i64,
    result: MatchResult,
    opponent: i64,
    seed: u32,
) -> InputMatch {
    let outcomes: Vec<bool> = match result {
        MatchResult::Win => vec![true, true],
        MatchResult::Loss => vec![false, false],
        MatchResult::Draw => vec![true, false],
    };
    let games: Vec<InputGame> = outcomes
        .iter()
        .enumerate()
        .map(|(n, won)| InputGame {
            won: Some(*won),
            on_play: Some((seed + n as u32).is_multiple_of(2)),
            mulligans: if seed.is_multiple_of(5) && n == 0 {
                1
            } else {
                0
            },
            turns: Some(5 + (seed + n as u32) % 6),
        })
        .collect();
    InputMatch {
        started_at_ms,
        ended_at_ms: Some(started_at_ms + 45 * MINUTE_MS),
        format_label: format.to_string(),
        limited: false,
        deck_id: Some(deck_id),
        result,
        opponent_archetype_id: Some(opponent),
        game_count: games.len() as u32,
        games,
    }
}

/// A day of the year (1 is 1 January, up to 334) at an hour, local time.
pub fn day_ms(tz: Tz, day_of_year: u32, hour: u32) -> i64 {
    // Local days can be 23 or 25 hours long, so step by calendar date.
    let date = chrono::NaiveDate::from_yo_opt(2026, day_of_year).expect("a day of 2026");
    tz.from_local_datetime(&date.and_hms_opt(hour, 0, 0).expect("an hour"))
        .earliest()
        .expect("a local time")
        .timestamp_millis()
}

/// Thirty matches, one deck, one format, no card rows: a player just over the
/// floor. One draw among them.
pub fn light() -> WrappedInput {
    let tz = chrono_tz::Europe::London;
    let mut input = empty(tz);
    input.decks.push(deck(
        1,
        "Mono Red Burn",
        "Modern",
        &["R"],
        &["Lightning Bolt", "Goblin Guide", "Monastery Swiftspear"],
    ));
    for n in 0..30u32 {
        let result = match n {
            0 => MatchResult::Draw,
            1..=11 => MatchResult::Loss,
            _ => MatchResult::Win,
        };
        // One match a week at eight in the evening.
        let start = day_ms(tz, 5 + n * 7, 20);
        input
            .matches
            .push(plain_match(start, "Modern", Some(1), result));
    }
    input
}

/// A busy multi-format player: 600 matches over three kinds, game rows for
/// the main deck, card rows, runs, a Challenge Top 8 and two drafts.
pub fn grinder() -> WrappedInput {
    let tz = chrono_tz::America::New_York;
    let mut input = empty(tz);
    input.first_year = false;
    input.decks.push(deck(
        1,
        "Boros Energy",
        "Modern",
        &["R", "W"],
        &[
            "Ragavan, Nimble Pilferer",
            "Guide of Souls",
            "Ocelot Pride",
            "Galvanic Discharge",
            "Phlage, Titan of Fire's Fury",
        ],
    ));
    input.decks.push(deck(
        2,
        "Dimir Faeries",
        "Pauper",
        &["U", "B"],
        &["Spellstutter Sprite", "Counterspell"],
    ));
    input.archetypes.push(archetype(
        1,
        "Domain Zoo",
        &["R", "G", "W"],
        &["Scion of Draco", "Territorial Kavu"],
    ));
    input.archetypes.push(archetype(
        2,
        "Living End",
        &["B", "G"],
        &["Violent Outburst", "Grief"],
    ));
    input.archetypes.push(archetype(
        3,
        "Eldrazi Ramp",
        &[],
        &["Thought-Knot Seer", "Ugin, the Spirit Dragon"],
    ));
    input.archetypes.push(archetype(
        4,
        "Amulet Titan",
        &["G"],
        &["Primeval Titan", "Amulet of Vigor"],
    ));

    for n in 0..600u32 {
        let day = 1 + (n * 334) / 600;
        let hour = 19 + n % 5; // evenings into night
        let start = day_ms(tz, day.min(334), hour);
        let opponent = i64::from(n % 4) + 1;
        let m = if n < 400 {
            // Modern, the main deck. Domain Zoo mostly loses to us, Eldrazi Ramp mostly beats us.
            let result = match opponent {
                _ if n % 50 == 49 => MatchResult::Draw,
                1 if n % 3 == 0 => MatchResult::Loss,
                1 => MatchResult::Win,
                3 if n % 5 == 2 => MatchResult::Win,
                3 => MatchResult::Loss,
                _ if n % 7 == 0 => MatchResult::Loss,
                _ => MatchResult::Win,
            };
            game_match(start, "Modern", 1, result, opponent, n)
        } else if n < 550 {
            let result = if n % 2 == 0 {
                MatchResult::Win
            } else {
                MatchResult::Loss
            };
            game_match(start, "Pauper", 2, result, opponent, n)
        } else {
            let result = if n % 5 < 3 {
                MatchResult::Win
            } else {
                MatchResult::Loss
            };
            plain_match(start, "Limited", None, result)
        };
        input.matches.push(m);
    }

    input.deck_games.push(InputDeckGames {
        deck_id: 1,
        games: 800,
        won: 480,
        opening_hand_games: 780,
        postboard_games: 400,
    });
    input.deck_games.push(InputDeckGames {
        deck_id: 2,
        games: 300,
        won: 140,
        opening_hand_games: 290,
        postboard_games: 150,
    });
    input.cards.push(card(
        1,
        "Phlage, Titan of Fire's Fury",
        false,
        (150, 105),
        120,
        0,
    ));
    input.cards.push(card(
        1,
        "Ragavan, Nimble Pilferer",
        false,
        (300, 195),
        366,
        0,
    ));
    input
        .cards
        .push(card(1, "Ocelot Pride", false, (200, 124), 431, 0));
    input
        .cards
        .push(card(1, "Guide of Souls", false, (250, 150), 480, 0));
    input
        .cards
        .push(card(1, "Galvanic Discharge", false, (180, 114), 402, 0));
    input
        .cards
        .push(card(1, "Sacred Foundry", true, (700, 450), 500, 0));
    input
        .cards
        .push(card(1, "Wear // Tear", false, (12, 9), 40, 71));
    input
        .cards
        .push(card(1, "Blood Moon", false, (10, 6), 30, 58));
    input
        .cards
        .push(card(1, "Surgical Extraction", false, (8, 4), 20, 32));
    input
        .cards
        .push(card(1, "Wrath of the Skies", false, (6, 3), 12, 20));
    input
        .cards
        .push(card(2, "Counterspell", false, (20, 12), 90, 0));

    input.runs.push(InputRun {
        deck_id: Some(1),
        limited: false,
        wins: 5,
        losses: 0,
        trophy: true,
    });
    input.runs.push(InputRun {
        deck_id: Some(1),
        limited: false,
        wins: 5,
        losses: 0,
        trophy: true,
    });
    input.runs.push(InputRun {
        deck_id: Some(2),
        limited: false,
        wins: 3,
        losses: 2,
        trophy: false,
    });
    input.finishes.push(InputFinish {
        top_cut: true,
        text: "Top 8, Modern Challenge, 14 Sep".to_string(),
        finished_at_ms: local_ms(tz, 2026, 9, 14, 23, 0),
    });
    input.drafts.push(InputDraft {
        colour_pair: Some("Selesnya".to_string()),
        wins: 3,
        losses: 0,
    });
    input.drafts.push(InputDraft {
        colour_pair: Some("Selesnya".to_string()),
        wins: 1,
        losses: 2,
    });
    input
}

/// A player who only drafts: 60 limited matches, six drafts and a trophy run.
pub fn limited() -> WrappedInput {
    let tz = chrono_tz::America::Los_Angeles;
    let mut input = empty(tz);
    for n in 0..60u32 {
        let result = if n % 3 == 2 {
            MatchResult::Loss
        } else {
            MatchResult::Win
        };
        // Saturday and Sunday mornings: day 3 of 2026 is a Saturday.
        let weekend = n / 2;
        let day = 3 + weekend * 7 + (n % 2);
        let start = day_ms(tz, day.min(334), 9 + n % 3);
        input
            .matches
            .push(plain_match(start, "Limited", None, result));
    }
    let pairs = [
        "Selesnya", "Selesnya", "Izzet", "Selesnya", "Golgari", "Izzet",
    ];
    let records = [(3, 0), (2, 2), (1, 3), (3, 1), (0, 3), (2, 3)];
    for (pair, (wins, losses)) in pairs.iter().zip(records) {
        input.drafts.push(InputDraft {
            colour_pair: Some(pair.to_string()),
            wins,
            losses,
        });
    }
    input.runs.push(InputRun {
        deck_id: None,
        limited: true,
        wins: 3,
        losses: 0,
        trophy: true,
    });
    input
}
