//! The Wrapped bundle: everything the page needs, already decided. A card is
//! in `cards` only when it passed its floor and its celebrate rule; the page
//! never decides whether a card shows. Opponent names, account names and match
//! tokens never appear here.

use serde::{Deserialize, Serialize};

/// Raised whenever the bundle's shape changes in a way a reader must know.
pub const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WrappedBundle {
    pub version: u32,
    pub year: i32,
    pub window: YearWindow,
    #[specta(type = specta_typescript::Number)]
    pub built_at_ms: i64,
    /// The player's first MyMTGO Wrapped: the opening card says it may look
    /// light.
    pub first_year: bool,
    pub totals: Totals,
    pub cards: Vec<WrappedCard>,
}

/// The counted year: from 1 January (inclusive) to 1 December (exclusive),
/// local midnights, as UTC milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct YearWindow {
    #[specta(type = specta_typescript::Number)]
    pub start_ms: i64,
    #[specta(type = specta_typescript::Number)]
    pub end_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub matches: u32,
    pub games: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub hours: u32,
}

/// A card or deck image, from the same three sources the app's deck views
/// use: the remote art crop, the downloaded file and the deck's own cover.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Art {
    pub remote: Option<String>,
    pub local: Option<String>,
    pub custom: Option<String>,
}

/// One card of the story, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum WrappedCard {
    MagicYear(MagicYearCard),
    Banned(BannedCard),
    YearInNumbers(YearInNumbersCard),
    Formats(FormatsCard),
    WhenYouPlay(WhenYouPlayCard),
    DeckOfYear(DeckCard),
    BestDeck(DeckCard),
    MvpCard(MvpCardCard),
    OpeningHand(CardCountCard),
    SideboardHero(CardCountCard),
    MostFaced(MostFacedCard),
    BestMatchup(MatchupCard),
    ToughestRival(RivalCard),
    Comebacks(ComebacksCard),
    PlayDraw(PlayDrawCard),
    Trophies(TrophiesCard),
    Limited(LimitedCard),
    PlayerType(PlayerTypeCard),
    Wrap(WrapCard),
}

/// The opening card: the year in Magic itself (sets, bans, Pro Tours,
/// qualifier seasons). The page holds those facts per year, so the card
/// carries nothing; it marks where the story starts.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MagicYearCard {}

/// The year's banned and restricted cards, shown as the cards themselves.
/// Like `MagicYearCard`, the page holds the facts per year.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct BannedCard {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct YearInNumbersCard {
    pub matches: u32,
    pub games: u32,
    pub hours: u32,
    pub days_played: u32,
    /// Matches started on each local day of the counted year, 1 January
    /// first, one entry per day up to 30 November: the card's activity grid.
    pub by_day: Vec<u32>,
    /// The player's place among MyMTGO players by matches played, as a
    /// "top N%" (1 to 100). Set from the API's percentiles (phase 4); the
    /// card only shows it when it is 50 or less.
    pub top_percent: Option<u32>,
}

/// The player's format of choice front and centre, the rest as honourable
/// mentions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FormatsCard {
    pub favourite: FavouriteFormat,
    /// The other formats played, most matches first; empty when the player
    /// stuck to one.
    pub mentions: Vec<FormatShare>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FavouriteFormat {
    pub label: String,
    pub matches: u32,
    /// 0 to 100.
    pub percent: u32,
    /// The art of the player's most played deck in this format.
    pub art: Art,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FormatShare {
    pub label: String,
    pub matches: u32,
    /// 0 to 100.
    pub percent: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WhenYouPlayCard {
    /// 0 is Monday.
    pub peak_weekday: u8,
    /// 0 to 23, local time.
    pub peak_hour: u8,
    /// 1 to 12.
    pub busiest_month: u8,
    /// Matches started in each local hour, 24 entries.
    pub by_hour: Vec<u32>,
    /// The part of the day with the most matches (`when::play_period`); the
    /// card's scene and name.
    pub period: PlayPeriod,
    /// An average Saturday or Sunday beat an average weekday
    /// (`when::prefers_weekends`).
    pub weekends: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum PlayPeriod {
    /// 5am to noon.
    Morning,
    /// Noon to 5pm.
    Afternoon,
    /// 5pm to 10pm.
    Evening,
    /// 10pm to 5am.
    Night,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckCard {
    pub name: String,
    pub format_label: String,
    pub matches: u32,
    pub wins: u32,
    pub losses: u32,
    /// 0 to 100; only when 50 or more.
    pub win_rate: Option<u32>,
    pub trophies: u32,
    pub art: Art,
    /// Colour identity letters in WUBRG order, e.g. `["R", "W"]`; empty for
    /// colourless.
    pub colors: Vec<String>,
    /// Up to five of the deck's key cards (its most played non-land cards),
    /// shown in a row.
    pub cards: Vec<KeyCard>,
    /// The player's place among MyMTGO players of the deck's archetype by
    /// matches played (the API's percentiles, phase 4). Never by win rate:
    /// Wrapped ranks how much, not how well.
    pub rank: Option<DeckRank>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckRank {
    /// The archetype's name, which may differ from the deck's own name.
    pub archetype: String,
    /// "Top N%", 1 to 100; the card only shows it when it is 25 or less.
    pub top_percent: u32,
}

/// One card of a deck, by name, with its full card image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct KeyCard {
    pub name: String,
    pub image: Art,
}

/// The player's MVP: the card with the highest personal impact (the API's
/// Impact measure, made personal): the player's win rate in games they cast
/// it, minus their win rate in all games with the decks that ran it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MvpCardCard {
    pub name: String,
    /// Times cast.
    pub cast: u32,
    /// Games it was cast in.
    pub games: u32,
    /// Win rate in games where it was cast; only when 50 or more.
    pub win_rate: Option<u32>,
    /// Percentage points the card added, always above 0 (the card is not
    /// made otherwise).
    pub impact: u32,
    /// The art crop, faint behind the card.
    pub art: Art,
    /// The full card image, the card's centrepiece.
    pub image: Art,
    /// Honourable mentions: the next cards by the same measure, best first,
    /// at most three.
    pub runners_up: Vec<RunnerUp>,
}

/// A card just behind the one a card features: its name and its number by the
/// same measure (impact points, opening hands or times sided in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RunnerUp {
    pub name: String,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardCountCard {
    pub name: String,
    /// Opening hands it was in, or times it was sided in.
    pub count: u32,
    /// Out of: games with a known opening hand, or games after sideboarding.
    pub games: u32,
    /// The art crop, faint behind the card.
    pub art: Art,
    /// The full card image, the card's centrepiece.
    pub image: Art,
    /// Honourable mentions: the next cards by the same measure, best first,
    /// at most three.
    pub runners_up: Vec<RunnerUp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MostFacedCard {
    pub archetype: String,
    pub matches: u32,
    pub unique_archetypes: u32,
    pub look: ArchetypeLook,
}

/// How an opponent's archetype is drawn, as on the Metagame page's tile:
/// one of its cards' art, its colours and the cards it is built on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeLook {
    pub art: Art,
    /// Colour identity letters in WUBRG order; empty for colourless.
    pub colors: Vec<String>,
    /// Up to five of the archetype's key cards.
    pub cards: Vec<KeyCard>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchupCard {
    pub archetype: String,
    pub matches: u32,
    pub wins: u32,
    /// 0 to 100.
    pub win_rate: u32,
    pub look: ArchetypeLook,
}

/// The archetype that beat the player most often. It carries no win rate:
/// the card celebrates the wins, never the losing record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RivalCard {
    pub archetype: String,
    pub matches: u32,
    pub wins: u32,
    pub look: ArchetypeLook,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ComebacksCard {
    pub wins_after_mulligan: u32,
    pub quickest_win_turns: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum PlaySide {
    Play,
    Draw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlayDrawCard {
    /// The better side; the card only celebrates.
    pub side: PlaySide,
    /// 0 to 100; the card is only made when it is 50 or more.
    pub win_rate: u32,
    pub games: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TrophiesCard {
    pub league_trophies: u32,
    pub league_runs: u32,
    pub top8s: u32,
    pub best_finish: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LimitedCard {
    pub drafts: u32,
    pub colour_pair: Option<String>,
    pub best_record: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum PlayerType {
    Grinder,
    DeckLoyalist,
    ComebackKid,
    LimitedLover,
    FormatExplorer,
    AllRounder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlayerTypeCard {
    pub label: PlayerType,
    pub pills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct WrapCard {}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck() -> DeckCard {
        DeckCard {
            name: "Boros Energy".to_string(),
            format_label: "Modern".to_string(),
            matches: 412,
            wins: 239,
            losses: 173,
            win_rate: Some(58),
            trophies: 1,
            art: Art::default(),
            colors: vec!["R".to_string(), "W".to_string()],
            cards: vec![KeyCard {
                name: "Phlage, Titan of Fire's Fury".to_string(),
                image: Art::default(),
            }],
            rank: None,
        }
    }

    #[test]
    fn a_card_serialises_with_its_kind_in_camel_case() {
        let card = WrappedCard::DeckOfYear(deck());
        let value = serde_json::to_value(&card).expect("serialise");
        assert_eq!(value["kind"], "deckOfYear");
        assert_eq!(value["formatLabel"], "Modern");
        assert_eq!(value["winRate"], 58);
        let back: WrappedCard = serde_json::from_value(value).expect("deserialise");
        assert_eq!(back, card);
    }

    #[test]
    fn an_empty_card_still_carries_its_kind() {
        let value = serde_json::to_value(WrappedCard::Wrap(WrapCard {})).expect("serialise");
        assert_eq!(value, serde_json::json!({ "kind": "wrap" }));
        let value =
            serde_json::to_value(WrappedCard::MagicYear(MagicYearCard {})).expect("serialise");
        assert_eq!(value, serde_json::json!({ "kind": "magicYear" }));
        let value = serde_json::to_value(WrappedCard::Banned(BannedCard {})).expect("serialise");
        assert_eq!(value, serde_json::json!({ "kind": "banned" }));
    }

    #[test]
    fn the_numbers_card_carries_a_count_per_day() {
        let card = WrappedCard::YearInNumbers(YearInNumbersCard {
            matches: 3,
            games: 7,
            hours: 2,
            days_played: 2,
            by_day: vec![2, 0, 1],
            top_percent: None,
        });
        let value = serde_json::to_value(&card).expect("serialise");
        assert_eq!(value["byDay"], serde_json::json!([2, 0, 1]));
    }
}
