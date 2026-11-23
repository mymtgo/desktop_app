//! The deck dashboard's standout cards (0.x `GetStandoutCards`, cards survey
//! section 11): six picks from a deck's card stats, each with its stat text.

use crate::stats::CardStat;

/// Games a card must be cast in to be the top performer.
pub const TOP_PERFORMER_MIN_GAMES: u32 = 20;

/// Postboard games a card needs to be a most sided in or out pick.
pub const SIDEBOARD_MIN_GAMES: u32 = 3;

/// One pick: the card and its stat text ("80% cast win rate").
#[derive(Debug, Clone, PartialEq)]
pub struct StandoutCard {
    pub name: String,
    pub image: Option<String>,
    pub local_image: Option<String>,
    pub stat: String,
}

/// The six picks; `None` where the deck has no card that qualifies.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Standout {
    pub top_performer: Option<StandoutCard>,
    pub most_cast: Option<StandoutCard>,
    pub most_seen: Option<StandoutCard>,
    pub most_played_land: Option<StandoutCard>,
    pub most_sided_in: Option<StandoutCard>,
    pub most_sided_out: Option<StandoutCard>,
}

/// The six picks from `stats` (the deck's local card stats, in the order the
/// store returns them: type, then name).
pub fn standout_cards(stats: &[CardStat]) -> Standout {
    // 0.x `str_contains($type, 'Land')`: case-sensitive, no type is no land.
    let is_land = |card: &CardStat| {
        card.card_type
            .as_deref()
            .is_some_and(|t| t.contains("Land"))
    };
    let non_land_maindeck = |card: &&CardStat| !card.is_sideboard && !is_land(card);

    let top_performer = best(
        stats
            .iter()
            .filter(non_land_maindeck)
            .filter(|c| c.cast_games >= TOP_PERFORMER_MIN_GAMES),
        |c| f64::from(c.cast_won) / f64::from(c.cast_games),
    )
    .map(|c| {
        let text = format!("{}% cast win rate", whole_percent(c.cast_won, c.cast_games));
        pick(c, text)
    });

    let most_cast = best(
        stats
            .iter()
            .filter(non_land_maindeck)
            .filter(|c| c.cast_games > 0),
        |c| f64::from(c.cast_games),
    )
    .map(|c| pick(c, of_games(c.cast_games, c.total_games, "games")));

    let most_seen = best(
        stats
            .iter()
            .filter(non_land_maindeck)
            .filter(|c| c.seen_games > 0),
        |c| f64::from(c.seen_games),
    )
    .map(|c| pick(c, of_games(c.seen_games, c.total_games, "games")));

    let most_played_land = best(
        stats
            .iter()
            .filter(|c| !c.is_sideboard && is_land(c) && c.played_games > 0),
        |c| f64::from(c.played_games),
    )
    .map(|c| pick(c, of_games(c.played_games, c.total_games, "games")));

    let most_sided_in = best(
        stats.iter().filter(|c| {
            c.is_sideboard && c.sided_in_games > 0 && c.postboard_games >= SIDEBOARD_MIN_GAMES
        }),
        |c| f64::from(c.sided_in_games),
    )
    .map(|c| {
        pick(
            c,
            of_games(c.sided_in_games, c.postboard_games, "postboard games"),
        )
    });

    let most_sided_out = best(
        stats
            .iter()
            .filter(non_land_maindeck)
            .filter(|c| c.sided_out_games > 0 && c.postboard_games >= SIDEBOARD_MIN_GAMES),
        |c| f64::from(c.sided_out_games),
    )
    .map(|c| {
        pick(
            c,
            of_games(c.sided_out_games, c.postboard_games, "postboard games"),
        )
    });

    Standout {
        top_performer,
        most_cast,
        most_seen,
        most_played_land,
        most_sided_in,
        most_sided_out,
    }
}

/// The card with the highest `key`; the first one wins a tie (0.x's stable
/// `sortByDesc(...)->first()`).
fn best<'a>(
    cards: impl Iterator<Item = &'a CardStat>,
    key: impl Fn(&CardStat) -> f64,
) -> Option<&'a CardStat> {
    let mut winner: Option<(&CardStat, f64)> = None;
    for card in cards {
        let value = key(card);
        match winner {
            Some((_, top)) if value <= top => {}
            _ => winner = Some((card, value)),
        }
    }
    winner.map(|(card, _)| card)
}

fn pick(card: &CardStat, stat: String) -> StandoutCard {
    StandoutCard {
        name: card.name.clone(),
        image: card.image.clone(),
        local_image: card.local_image.clone(),
        stat,
    }
}

fn of_games(count: u32, total: u32, unit: &str) -> String {
    format!("{count} of {total} {unit}")
}

/// 0.x `GetStandoutCards`: `SampleRate::of(...)->percentage() ?? 0`.
fn whole_percent(count: u32, games: u32) -> u32 {
    decks::stats::sample_rate_percent(count, games).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, card_type: &str) -> CardStat {
        CardStat {
            key: name.to_string(),
            name: name.to_string(),
            card_type: Some(card_type.to_string()),
            ..CardStat::default()
        }
    }

    /// `games` games cast, of which `won` were won (0.x test: one row per game).
    fn cast_in(name: &str, games: u32, won: u32) -> CardStat {
        CardStat {
            total_games: games,
            cast_games: games,
            cast_won: won,
            cast_lost: games - won,
            ..card(name, "Instant")
        }
    }

    #[test]
    fn top_performer_ranks_by_cast_win_rate_over_games_cast() {
        // Cast 20 games, 10 won: 50%. A solid card: 25 games, 20 won: 80%.
        let stats = vec![cast_in("Noisy Card", 20, 10), cast_in("Solid Card", 25, 20)];
        let picks = standout_cards(&stats);
        let top = picks.top_performer.expect("a top performer");
        assert_eq!(top.name, "Solid Card");
        assert_eq!(top.stat, "80% cast win rate");
    }

    #[test]
    fn top_performer_needs_twenty_games_cast() {
        let stats = vec![cast_in("Hot Streak", 5, 5), cast_in("Reliable", 30, 18)];
        let picks = standout_cards(&stats);
        assert_eq!(picks.top_performer.expect("top").name, "Reliable");

        let only_small = standout_cards(&[cast_in("Hot Streak", 19, 19)]);
        assert_eq!(only_small.top_performer, None);
    }

    #[test]
    fn most_cast_ranks_by_games_cast_and_says_of_total() {
        let stats = vec![
            CardStat {
                total_games: 5,
                cast_games: 5,
                total_cast: 20,
                ..card("Rare But Spammy", "Instant")
            },
            CardStat {
                total_games: 10,
                cast_games: 10,
                total_cast: 10,
                ..card("Frequent", "Instant")
            },
        ];
        let picks = standout_cards(&stats);
        let most = picks.most_cast.expect("most cast");
        assert_eq!(most.name, "Frequent");
        assert_eq!(most.stat, "10 of 10 games");
    }

    #[test]
    fn most_played_land_ranks_by_games_played_not_seen() {
        let stats = vec![
            CardStat {
                total_games: 10,
                seen_games: 10,
                played_games: 2,
                ..card("Mountain", "Basic Land")
            },
            CardStat {
                total_games: 8,
                seen_games: 8,
                played_games: 8,
                ..card("Forest", "Basic Land")
            },
        ];
        let picks = standout_cards(&stats);
        let land = picks.most_played_land.expect("land");
        assert_eq!(land.name, "Forest");
        assert_eq!(land.stat, "8 of 8 games");
    }

    #[test]
    fn most_seen_says_seen_games_over_total_games() {
        let stats = vec![CardStat {
            total_games: 7,
            seen_games: 7,
            total_seen: 14,
            ..card("Visible", "Creature")
        }];
        let picks = standout_cards(&stats);
        let seen = picks.most_seen.expect("seen");
        assert_eq!(seen.name, "Visible");
        assert_eq!(seen.stat, "7 of 7 games");
    }

    #[test]
    fn sideboard_picks_need_three_postboard_games() {
        let mut side = card("Qsweeper", "Sorcery");
        side.is_sideboard = true;
        side.sided_in_games = 2;
        side.postboard_games = 2;
        let mut cut = card("Qcut", "Creature");
        cut.sided_out_games = 3;
        cut.postboard_games = 4;
        let picks = standout_cards(&[side.clone(), cut]);
        assert_eq!(picks.most_sided_in, None);
        let out = picks.most_sided_out.expect("sided out");
        assert_eq!(out.name, "Qcut");
        assert_eq!(out.stat, "3 of 4 postboard games");

        side.sided_in_games = 3;
        side.postboard_games = 3;
        let picks = standout_cards(&[side]);
        assert_eq!(
            picks.most_sided_in.expect("sided in").stat,
            "3 of 3 postboard games"
        );
    }

    #[test]
    fn lands_and_sideboard_cards_are_left_out_of_the_maindeck_picks() {
        let mut side = cast_in("Qside", 30, 30);
        side.is_sideboard = true;
        side.seen_games = 30;
        let mut land = card("Qland", "Land");
        land.total_games = 30;
        land.cast_games = 30;
        land.cast_won = 30;
        land.seen_games = 30;
        let picks = standout_cards(&[side, land]);
        assert_eq!(picks.top_performer, None);
        assert_eq!(picks.most_cast, None);
        assert_eq!(picks.most_seen, None);
    }

    #[test]
    fn a_tie_keeps_the_first_card_in_store_order() {
        let stats = vec![cast_in("Qfirst", 10, 5), cast_in("Qsecond", 10, 5)];
        assert_eq!(
            standout_cards(&stats).most_cast.expect("cast").name,
            "Qfirst"
        );
    }
}
