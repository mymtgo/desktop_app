//! Card facts, the Cards page rows (0.x `BuildLimitedCardRows`) and the
//! cross-draft card stats (0.x `ComputeCrossDraftCardStats`).
//!
//! Card facts come in as a map keyed by catalog id, built by the caller from
//! the `cards` table. A missing entry is neutral: the card shows as `#id`, has
//! no colours, no oracle id and is never a basic land.

use std::collections::BTreeMap;

use crate::draft::{Draft, Pick};
use crate::evolution::{CardStatus, pool_statuses};
use crate::pool::{RegisteredCard, RegisteredDeck, run_pool};
use crate::wheel::{seats, seen_wheel};

/// The five colours, in 0.x's WUBRG order (`cards::colors::WUBRG`).
pub const COLORS: [char; 5] = ::cards::colors::WUBRG;

/// What the limited pages know about one card (a `cards` row).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CardFacts {
    pub name: Option<String>,
    /// The `type` column (`Creature`, `Basic Land`, ...); `None` until the
    /// card data is filled.
    pub card_type: Option<String>,
    /// Colour letters, `UG` or `U,G`; anything but WUBRG is ignored.
    pub colors: Option<String>,
    pub color_identity: Option<String>,
    pub rarity: Option<String>,
    pub set_code: Option<String>,
    pub set_name: Option<String>,
    pub oracle_id: Option<String>,
    pub cmc: Option<f64>,
}

/// Card facts by catalog id.
pub type CardFactsMap = BTreeMap<u32, CardFacts>;

/// The card's name, or `#id` when it has no card data (0.x
/// `LimitedCardData::fromCatalog`).
pub fn card_name(cards: &CardFactsMap, catalog_id: u32) -> String {
    match cards.get(&catalog_id).and_then(|card| card.name.clone()) {
        Some(name) => name,
        None => format!("#{catalog_id}"),
    }
}

/// True when the card is known to be a basic land. Unknown cards are not.
pub fn is_basic(cards: &CardFactsMap, catalog_id: u32) -> bool {
    cards
        .get(&catalog_id)
        .is_some_and(|card| card.card_type.as_deref() == Some("Basic Land"))
}

/// The card's WUBRG letters in the order its `colors` lists them (0.x
/// `array_intersect(str_split(colors), COLORS)`). Unknown cards have none.
pub(crate) fn colour_letters(cards: &CardFactsMap, catalog_id: u32) -> Vec<char> {
    let colors = cards
        .get(&catalog_id)
        .and_then(|card| card.colors.as_deref())
        .unwrap_or("");
    colors
        .chars()
        .filter(|letter| COLORS.contains(letter))
        .collect()
}

/// The card's oracle id, when it has a non-empty one.
pub(crate) fn oracle_of(cards: &CardFactsMap, catalog_id: u32) -> Option<&str> {
    cards
        .get(&catalog_id)
        .and_then(|card| card.oracle_id.as_deref())
        .filter(|oracle| !oracle.is_empty())
}

/// How a card fared in games across a run's deck (0.x `GetCardGameStats`
/// `castGames`, `castWon`, `castLost`), keyed by oracle id by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CastStats {
    pub games_cast: u32,
    pub cast_won: u32,
    pub cast_lost: u32,
}

/// One row of the Cards page.
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedCardRow {
    pub catalog_id: u32,
    pub oracle_id: Option<String>,
    /// The ordinals this card was picked at, ascending (empty for sealed).
    pub ordinals: Vec<u32>,
    /// `P{pack}p{pick}` per pick, in pick order.
    pub labels: Vec<String>,
    pub status: CardStatus,
    /// Sealed only: some copies came from the booster added mid-run.
    pub added: bool,
    pub games_cast: u32,
    pub cast_won: u32,
    pub cast_lost: u32,
    /// `round(won / (won + lost) * 100)`; `None` with no decided game.
    pub win_pct_cast: Option<i64>,
    pub seen_count: u32,
    pub wheeled: bool,
    pub prior_taken: u32,
    pub prior_avg_ordinal: Option<f64>,
    pub prior_wheeled: u32,
    pub prior_drafts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CardRowsSummary {
    pub distinct: usize,
    /// Games played with the run's deck (an input).
    pub games: u32,
    /// The most other drafts any card was seen in.
    pub other_drafts: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LimitedCardRows {
    pub rows: Vec<LimitedCardRow>,
    pub summary: CardRowsSummary,
}

/// Everything the Cards page needs about one run.
#[derive(Debug, Clone, Copy)]
pub struct CardRowsInput<'a> {
    pub is_sealed: bool,
    /// The run's draft, if it has one.
    pub draft: Option<&'a Draft>,
    /// The run's registered decks.
    pub decks: &'a [RegisteredDeck],
    /// The pool Arena stated for the run, one catalog id per card.
    pub stated_pool: Option<&'a [u32]>,
    /// `cross_draft_card_stats` for the run.
    pub cross_draft: &'a BTreeMap<String, CrossDraftStats>,
    /// Cast stats by oracle id over the run's games.
    pub cast_stats: &'a BTreeMap<String, CastStats>,
    /// How many games the run's matches had.
    pub games_played: u32,
    pub cards: &'a CardFactsMap,
}

/// The Cards page rows (0.x `BuildLimitedCardRows`): one row per distinct
/// card picked, in first-pick order. A sealed run with no draft, or a run
/// with a stated pool and no draft (an Arena draft), lists its pool instead;
/// any other run with no draft has no rows.
pub fn limited_card_rows(input: &CardRowsInput) -> LimitedCardRows {
    match input.draft {
        Some(draft) => draft_rows(input, draft),
        None if input.is_sealed || input.stated_pool.is_some() => sealed_rows(input),
        None => LimitedCardRows::default(),
    }
}

fn draft_rows(input: &CardRowsInput, draft: &Draft) -> LimitedCardRows {
    let seen = seen_wheel(&draft.picks, seats(draft.seat_count));
    let pool = run_pool(
        input.is_sealed,
        Some(draft),
        input.decks,
        input.cards,
        input.stated_pool,
    );
    let statuses = pool_statuses(&pool, input.decks, input.cards);

    // The picks of each card, in pick order, keyed in first-pick order.
    let mut order: Vec<u32> = Vec::new();
    let mut picks_of: BTreeMap<u32, Vec<&Pick>> = BTreeMap::new();
    for pick in &draft.picks {
        let Some(catalog_id) = pick.picked_catalog_id else {
            continue;
        };
        if !picks_of.contains_key(&catalog_id) {
            order.push(catalog_id);
        }
        picks_of.entry(catalog_id).or_default().push(pick);
    }

    let mut rows = Vec::new();
    for catalog_id in order {
        let mut picks = picks_of.remove(&catalog_id).unwrap_or_default();
        picks.sort_by_key(|pick| pick.ordinal);

        let mut row = base_row(input, catalog_id, &statuses);
        row.ordinals = picks.iter().map(|pick| pick.ordinal).collect();
        row.labels = picks
            .iter()
            .map(|pick| format!("P{}p{}", pick.pack_number, pick.pick_number))
            .collect();
        if let Some(facts) = seen.get(&catalog_id) {
            row.seen_count = facts.seen_count;
            row.wheeled = facts.wheeled;
        }
        if let Some(prior) = row
            .oracle_id
            .as_ref()
            .and_then(|oracle| input.cross_draft.get(oracle))
        {
            row.prior_taken = prior.times_taken;
            row.prior_avg_ordinal = prior.avg_ordinal;
            row.prior_wheeled = prior.times_wheeled;
            row.prior_drafts = prior.drafts;
        }
        rows.push(row);
    }
    // Stable, so cards first picked at the same ordinal keep their order.
    rows.sort_by_key(|row| row.ordinals.first().copied().unwrap_or(u32::MAX));

    let other_drafts = input
        .cross_draft
        .values()
        .map(|stats| stats.drafts)
        .max()
        .unwrap_or(0);

    LimitedCardRows {
        summary: CardRowsSummary {
            distinct: rows.len(),
            games: input.games_played,
            other_drafts,
        },
        rows,
    }
}

/// Sealed has no picks, so its rows are the pool it opened, with the booster
/// added mid-run flagged; an Arena draft's rows are its stated pool. Pick,
/// seen, wheel and prior facts stay empty.
fn sealed_rows(input: &CardRowsInput) -> LimitedCardRows {
    let pool = run_pool(true, None, input.decks, input.cards, input.stated_pool);
    let statuses = pool_statuses(&pool, input.decks, input.cards);

    let rows: Vec<LimitedCardRow> = pool
        .pool
        .keys()
        .map(|&catalog_id| {
            let mut row = base_row(input, catalog_id, &statuses);
            row.added = pool.added.contains_key(&catalog_id);
            row
        })
        .collect();

    LimitedCardRows {
        summary: CardRowsSummary {
            distinct: rows.len(),
            games: input.games_played,
            other_drafts: 0,
        },
        rows,
    }
}

/// A row with the facts both kinds share: status and cast stats.
fn base_row(
    input: &CardRowsInput,
    catalog_id: u32,
    statuses: &BTreeMap<u32, CardStatus>,
) -> LimitedCardRow {
    let oracle_id = input
        .cards
        .get(&catalog_id)
        .and_then(|card| card.oracle_id.clone());
    let cast = oracle_id
        .as_ref()
        .and_then(|oracle| input.cast_stats.get(oracle))
        .copied()
        .unwrap_or_default();
    let decided = cast.cast_won + cast.cast_lost;
    // 0.x `BuildLimitedCardRows`: `round($castWon / $decided * 100)` (the
    // `SampleRate` order).
    let win_pct_cast = decks::stats::sample_rate_percent(cast.cast_won, decided).map(i64::from);

    LimitedCardRow {
        catalog_id,
        oracle_id,
        ordinals: Vec::new(),
        labels: Vec::new(),
        status: statuses
            .get(&catalog_id)
            .copied()
            .unwrap_or(CardStatus::Cut),
        added: false,
        games_cast: cast.games_cast,
        cast_won: cast.cast_won,
        cast_lost: cast.cast_lost,
        win_pct_cast,
        seen_count: 0,
        wheeled: false,
        prior_taken: 0,
        prior_avg_ordinal: None,
        prior_wheeled: 0,
        prior_drafts: 0,
    }
}

/// Another draft the user made, with the run it belongs to.
#[derive(Debug, Clone, Copy)]
pub struct OtherDraft<'a> {
    /// The set code of the draft's run.
    pub set_code: Option<&'a str>,
    pub draft: &'a Draft,
    /// The run's last registered deck, if it has one.
    pub last_registered: Option<&'a [RegisteredCard]>,
}

/// How one card (by oracle id) fared in the user's other drafts of a set.
#[derive(Debug, Clone, PartialEq)]
pub struct CrossDraftStats {
    pub oracle_id: String,
    /// Drafts where the card was seen in a pack.
    pub drafts: u32,
    pub times_taken: u32,
    /// Mean pick ordinal, to one decimal; `None` when never picked.
    pub avg_ordinal: Option<f64>,
    pub times_passed: u32,
    pub times_wheeled: u32,
    /// Runs whose last registered main deck held it.
    pub made_deck: u32,
}

/// Per oracle id, how every card fared in the other drafts of `set_code`
/// (0.x `ComputeCrossDraftCardStats`). `others` are the user's drafts linked
/// to runs other than this one; drafts of another set are skipped. A run with
/// no set code compares against nothing. Cards without an oracle id are left
/// out.
pub fn cross_draft_card_stats(
    set_code: Option<&str>,
    others: &[OtherDraft],
    cards: &CardFactsMap,
) -> BTreeMap<String, CrossDraftStats> {
    let Some(set_code) = set_code.filter(|code| !code.is_empty()) else {
        return BTreeMap::new();
    };

    let mut stats: BTreeMap<String, CrossDraftStats> = BTreeMap::new();
    let mut ordinals: BTreeMap<String, Vec<u32>> = BTreeMap::new();

    for other in others
        .iter()
        .filter(|other| other.set_code == Some(set_code))
    {
        let draft = other.draft;
        let mut seen_oracles: Vec<String> = Vec::new();

        for (catalog_id, facts) in seen_wheel(&draft.picks, seats(draft.seat_count)) {
            let Some(oracle) = oracle_of(cards, catalog_id) else {
                continue;
            };
            let entry = stats_for(&mut stats, oracle);
            entry.times_taken += facts.picked_count;
            entry.times_passed += facts.passed_count;
            if facts.wheeled {
                entry.times_wheeled += 1;
            }
            if !seen_oracles.iter().any(|seen| seen == oracle) {
                seen_oracles.push(oracle.to_string());
            }
        }

        for pick in &draft.picks {
            let Some(catalog_id) = pick.picked_catalog_id else {
                continue;
            };
            if let Some(oracle) = oracle_of(cards, catalog_id) {
                // A committed pick can name a card that never appeared in the
                // pack it recorded, so the row may not exist yet.
                stats_for(&mut stats, oracle);
                ordinals
                    .entry(oracle.to_string())
                    .or_default()
                    .push(pick.ordinal);
            }
        }

        for oracle in &seen_oracles {
            stats_for(&mut stats, oracle).drafts += 1;
        }

        if let Some(registered) = other.last_registered {
            let mut main_oracles: Vec<&str> = Vec::new();
            for card in registered.iter().filter(|card| !card.sideboard) {
                if let Some(oracle) = oracle_of(cards, card.catalog_id)
                    && !main_oracles.contains(&oracle)
                {
                    main_oracles.push(oracle);
                }
            }
            for oracle in main_oracles {
                stats_for(&mut stats, oracle).made_deck += 1;
            }
        }
    }

    for (oracle, entry) in stats.iter_mut() {
        if let Some(picked) = ordinals.get(oracle) {
            let sum: u32 = picked.iter().sum();
            let mean = f64::from(sum) / picked.len() as f64;
            entry.avg_ordinal = Some(decks::php_round(mean, 1));
        }
    }
    stats
}

/// The stats row for an oracle id, created empty on first use.
fn stats_for<'a>(
    stats: &'a mut BTreeMap<String, CrossDraftStats>,
    oracle: &str,
) -> &'a mut CrossDraftStats {
    stats
        .entry(oracle.to_string())
        .or_insert_with(|| CrossDraftStats {
            oracle_id: oracle.to_string(),
            drafts: 0,
            times_taken: 0,
            avg_ordinal: None,
            times_passed: 0,
            times_wheeled: 0,
            made_deck: 0,
        })
}
