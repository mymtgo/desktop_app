//! What the UI receives (spec 6): one summary type for list rows and pushed
//! updates, so a pushed update patches its row in place. The score counts games
//! in both paths, so a row looks the same from memory or from storage.

use mtgo_match::{GameResult, MatchState, MatchView, Outcome};
use serde::Serialize;
use store::Platform;
use store::read::MatchRow;

use crate::platform::{BestOfView, PlatformView};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SummaryState {
    InProgress,
    Ended,
    Complete,
    Abandoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SummaryOutcome {
    Win,
    Loss,
    Draw,
    Unknown,
}

/// One matches list row (spec 6 columns).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchSummary {
    /// The `matches` row id. A pushed view has none; the list keeps the
    /// stored row's.
    pub id: Option<u32>,
    pub token: String,
    /// The opponent's display name (an Arena opponent's screen name).
    pub opponent: Option<String>,
    /// Display label, e.g. "Modern" for `CMODERN`.
    pub format: Option<String>,
    /// Display label (`match_type_label`), e.g. "Ranked" for Arena's `ranked`.
    pub match_type: Option<String>,
    /// The game the match was played on.
    pub platform: PlatformView,
    /// An Arena match's kind; `None` for MTGO.
    pub best_of: Option<BestOfView>,
    pub state: SummaryState,
    pub outcome: SummaryOutcome,
    /// Live hint while in progress; never stored.
    pub tentative_outcome: Option<SummaryOutcome>,
    pub wins: u32,
    pub losses: u32,
    pub needs_attention: bool,
    pub attention_reason: Option<String>,
    #[specta(type = specta_typescript::Number)]
    pub started_at_ms: i64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub ended_at_ms: Option<i64>,
    #[specta(type = specta_typescript::Number)]
    pub revision: u64,
    /// The player's note. A pushed view has none; the list keeps the stored
    /// row's note when a push replaces the row.
    pub notes: Option<String>,
    /// Each game won or lost, in game order (0.x `gameResults`). A game with
    /// no result keeps its place before a decided one; trailing ones are
    /// left out.
    pub game_results: Vec<GameResultView>,
    /// Entered by hand (0.x `manual`: the list shows the "M" badge).
    pub manual: bool,
    /// A Limited (draft or sealed) match by its format code: its row offers
    /// no "Set deck" (issue #332).
    pub limited: bool,
    /// A limited match's opponent colours, WUBRG letters comma separated
    /// (0.x `opponentColors`); only the Limited pages fill it.
    #[specta(optional)]
    pub opponent_colors: Option<String>,
    /// The opponent seat's archetype (archetypes spec section 12). A pushed
    /// view has none; the list keeps the stored row's.
    pub opponent_archetype: Option<OpponentArchetypeView>,
    /// The constructed rank a ranked Arena match was played at; `None` for
    /// MTGO, casual matches and ranked ones with no reading before them. A
    /// pushed view has none; the list keeps the stored row's.
    #[specta(optional)]
    pub rank: Option<MatchRankView>,
    /// An Endstep ranked match's rating and its change; `None` for other rows. A
    /// pushed view has none; the list keeps the stored row's.
    #[specta(optional)]
    pub rating: Option<RatingView>,
}

/// An Endstep match's rating before and after it. `delta` is the rating's
/// change, `after - before`: Endstep's own stored `delta` is the change of
/// its score, another scale, and is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RatingView {
    #[specta(type = specta_typescript::Number)]
    pub before: i64,
    #[specta(type = specta_typescript::Number)]
    pub after: i64,
    #[specta(type = specta_typescript::Number)]
    pub delta: i64,
}

impl RatingView {
    pub fn from_rating(rating: &store::read::MatchRating) -> Self {
        Self::from_before_after(rating.before, rating.after)
    }

    /// From a stored ranked result: only when both ratings are known.
    pub fn from_ranked(ranked: &store::endstep_sync::EndstepRanked) -> Option<Self> {
        Some(Self::from_before_after(
            ranked.rating_before?,
            ranked.rating_after?,
        ))
    }

    fn from_before_after(before: i64, after: i64) -> Self {
        Self {
            before,
            after,
            delta: after.saturating_sub(before),
        }
    }
}

/// The rank a match was played at, as the lists show it.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchRankView {
    pub class: String,
    pub level: u32,
    pub step: u32,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<u32>,
    /// "Bronze 2", "Mythic 85.4%" (`arena_match::rank_line::rank_label`).
    pub label: String,
}

impl MatchRankView {
    pub fn from_rank(rank: &store::read::MatchRank) -> Self {
        let reading = arena_match::rank_line::Observation {
            file_id: 0,
            offset: 0,
            at_ms: 0,
            class: rank.class.clone(),
            level: rank.level,
            step: rank.step,
            won: 0,
            lost: 0,
            mythic_percentile: rank.mythic_percentile,
            mythic_placement: rank.mythic_placement,
        };
        Self {
            class: rank.class.clone(),
            level: rank.level,
            step: rank.step,
            mythic_percentile: rank.mythic_percentile,
            mythic_placement: rank.mythic_placement,
            label: arena_match::rank_line::rank_label(&reading),
        }
    }
}

/// A match's opponent archetype as the lists and the match page show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OpponentArchetypeView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
    /// A commander archetype's cards (the opponent's commanders, partners
    /// both): hovering the name previews them. Empty for any other archetype.
    pub commander_cards: Vec<CommanderCardView>,
}

impl OpponentArchetypeView {
    pub fn from_seat(seat: &store::match_list::SeatArchetype) -> Self {
        Self {
            id: seat.id,
            name: seat.name.clone(),
            colors: archetype_colors(seat.color_identity.as_deref()),
            commander_cards: seat
                .commander_cards
                .iter()
                .map(CommanderCardView::from_card)
                .collect(),
        }
    }
}

/// A commander as the match page shows it: the full card image, readable;
/// its name and colours are the fallback without one. `Card <id>` and no
/// image without card data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CommanderCardView {
    pub catalog_id: u32,
    pub name: String,
    /// The full card image URL.
    pub image: Option<String>,
    /// The card image file in v1's images folder (`card` scheme).
    pub local_image: Option<String>,
    /// Colour identity letters, WUBRG order.
    pub colors: Vec<String>,
}

impl CommanderCardView {
    pub fn from_card(card: &store::match_list::CommanderCard) -> Self {
        let text = |value: &Option<String>| value.clone().filter(|text| !text.is_empty());
        Self {
            catalog_id: u32::try_from(card.catalog_id).unwrap_or_default(),
            name: text(&card.name).unwrap_or_else(|| format!("Card {}", card.catalog_id)),
            image: text(&card.image),
            local_image: text(&card.local_image),
            colors: archetype_colors(card.color_identity.as_deref()),
        }
    }
}

/// An archetype's stored colours as letters in WUBRG order.
pub fn archetype_colors(identity: Option<&str>) -> Vec<String> {
    let canonical = archetypes::normalize_colors(identity).unwrap_or_default();
    canonical
        .split(',')
        .filter(|letter| !letter.is_empty())
        .map(str::to_string)
        .collect()
}

/// One game of a list row (0.x `GameResultSummaryData`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameResultView {
    /// `null` for a game with no known winner, kept so the games after it
    /// stay in their places (a trailing one, the game being played, is left
    /// out).
    pub won: Option<bool>,
    /// `null` when the game's play or draw is unknown.
    pub on_play: Option<bool>,
}

/// One page of the matches list.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct MatchPage {
    pub rows: Vec<MatchSummary>,
    pub total: u32,
    pub page: u32,
    pub per_page: u32,
    /// Complete matches of the list's scope with no opponent archetype
    /// (0.x `unknownArchetypeCount`); 0 where a list has no archetypes.
    pub unknown_archetype_count: u32,
    /// The archetype filter's options with the scope's matches against
    /// each (0.x `archetypes`); empty where a list has no archetypes.
    pub archetypes: Vec<ArchetypeCountView>,
}

/// An archetype the list's filter offers, with the matches against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeCountView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    pub colors: Vec<String>,
    /// Homebrew or Rogue (shown as "System").
    pub is_fallback: bool,
    pub match_count: u32,
}

fn state_of(state: MatchState) -> SummaryState {
    match state {
        MatchState::InProgress => SummaryState::InProgress,
        MatchState::Ended => SummaryState::Ended,
        MatchState::Complete => SummaryState::Complete,
        MatchState::Abandoned => SummaryState::Abandoned,
    }
}

fn outcome_of(outcome: Outcome) -> SummaryOutcome {
    match outcome {
        Outcome::Win => SummaryOutcome::Win,
        Outcome::Loss => SummaryOutcome::Loss,
        Outcome::Draw => SummaryOutcome::Draw,
        Outcome::Unknown => SummaryOutcome::Unknown,
    }
}

/// The text `store::names` writes for each state.
pub(crate) fn state_from_str(text: &str) -> Option<SummaryState> {
    match text {
        "in_progress" => Some(SummaryState::InProgress),
        "ended" => Some(SummaryState::Ended),
        "complete" => Some(SummaryState::Complete),
        "abandoned" => Some(SummaryState::Abandoned),
        _ => None,
    }
}

pub(crate) fn outcome_from_str(text: &str) -> Option<SummaryOutcome> {
    match text {
        "win" => Some(SummaryOutcome::Win),
        "loss" => Some(SummaryOutcome::Loss),
        "draw" => Some(SummaryOutcome::Draw),
        "unknown" => Some(SummaryOutcome::Unknown),
        _ => None,
    }
}

/// 0.x `MtgoFormat::display`: drop the constructed `C` prefix (only when a
/// capital follows, so "Commander" stays), then title case. An Arena-only
/// code (`ARENA_HISTORIC`) drops its `ARENA_` prefix and reads its words in
/// title case.
pub fn format_label(code: Option<&str>) -> Option<String> {
    let code = code?.trim();
    if code.is_empty() {
        return None;
    }
    if let Some(rest) = code.strip_prefix("ARENA_")
        && !rest.is_empty()
    {
        return Some(title_case(&rest.replace('_', " ").to_lowercase()));
    }
    let mut chars = code.chars();
    let bare = match (chars.next(), chars.next()) {
        (Some('C'), Some(second)) if second.is_ascii_uppercase() => &code[1..],
        _ => code,
    };
    Some(title_case(&bare.to_lowercase()))
}

/// A match type as the lists show it: Arena's `ranked` and `unranked` are
/// capitalised; MTGO's types (`League`, `Constructed`) stay as stored.
pub fn match_type_label(text: Option<&str>) -> Option<String> {
    let text = text?;
    match text {
        "ranked" => Some("Ranked".to_string()),
        "unranked" => Some("Unranked".to_string()),
        _ => Some(text.to_string()),
    }
}

fn title_case(text: &str) -> String {
    let mut out = String::new();
    let mut start_of_word = true;
    for ch in text.chars() {
        if start_of_word {
            out.extend(ch.to_uppercase());
        } else {
            out.push(ch);
        }
        start_of_word = ch == ' ';
    }
    out
}

/// A pushed view of a match projected from the log, played on `platform`.
/// `opponent_name` is the opponent's display name when the view names its
/// opponent by registry key (Arena). The score counts games, like the
/// stored row.
pub fn from_view(
    view: &MatchView,
    revision: u64,
    platform: Platform,
    opponent_name: Option<String>,
) -> MatchSummary {
    let mut wins = 0;
    let mut losses = 0;
    let mut game_results = Vec::new();
    for game in &view.games {
        let won = match game.result {
            GameResult::Win => {
                wins += 1;
                Some(true)
            }
            GameResult::Loss => {
                losses += 1;
                Some(false)
            }
            GameResult::Unknown => None,
        };
        game_results.push(GameResultView {
            won,
            on_play: game.on_play,
        });
    }
    drop_trailing_unknown(&mut game_results);
    MatchSummary {
        id: None,
        token: view.token.clone(),
        opponent: opponent_name.or_else(|| view.opponent.clone()),
        format: format_label(view.format.as_deref()),
        match_type: match_type_label(view.match_type.as_deref()),
        platform: platform.into(),
        best_of: best_of_of(platform, view.game_structure.as_deref()),
        state: state_of(view.state),
        outcome: outcome_of(view.outcome),
        tentative_outcome: view.tentative_outcome.map(outcome_of),
        wins,
        losses,
        needs_attention: false,
        attention_reason: None,
        started_at_ms: view.started_at_ms,
        ended_at_ms: view.ended_at_ms,
        revision,
        notes: None,
        game_results,
        manual: false,
        limited: is_limited_format(view.format.as_deref()),
        opponent_colors: None,
        opponent_archetype: None,
        rank: None,
        rating: None,
    }
}

/// Leave out the games at the end with no known winner (the game being
/// played); an unknown game before a decided one keeps its place, so
/// Win, Unknown, Loss shows as G1 Win, G2 ?, G3 Loss.
fn drop_trailing_unknown(games: &mut Vec<GameResultView>) {
    while games.last().is_some_and(|game| game.won.is_none()) {
        games.pop();
    }
}

/// An Arena or Endstep match's kind from its `game_structure`. An MTGO
/// match holds a structure code there (`Modern`), so it has no kind.
fn best_of_of(platform: Platform, game_structure: Option<&str>) -> Option<BestOfView> {
    if platform == Platform::Mtgo {
        return None;
    }
    game_structure
        .and_then(store::BestOf::from_key)
        .map(BestOfView::from)
}

/// Whether a match's format code is a Limited one (`decks::is_limited_match_format`).
fn is_limited_format(code: Option<&str>) -> bool {
    code.is_some_and(|code| decks::is_limited_match_format(code.trim()))
}

/// A pushed view of a manual match (`store::manual::manual_view`); an Arena
/// opponent shows `opponent_name`, as `from_view`.
pub fn from_manual_view(
    view: &MatchView,
    revision: u64,
    platform: Platform,
    opponent_name: Option<String>,
) -> MatchSummary {
    MatchSummary {
        manual: true,
        ..from_view(view, revision, platform, opponent_name)
    }
}

/// A stored row. `None` for text the schema allows but the list cannot show
/// (`pending` is never written for a visible match).
pub fn from_row(row: &MatchRow) -> Option<MatchSummary> {
    let mut game_results = Vec::new();
    for game in &row.games {
        game_results.push(GameResultView {
            won: store::names::game_won(&game.result),
            on_play: game.on_play,
        });
    }
    drop_trailing_unknown(&mut game_results);
    Some(MatchSummary {
        id: u32::try_from(row.id).ok(),
        token: row.token.clone(),
        opponent: row.opponent.clone(),
        format: format_label(row.format.as_deref()),
        match_type: match_type_label(row.match_type.as_deref()),
        platform: row.platform.into(),
        best_of: best_of_of(row.platform, row.game_structure.as_deref()),
        state: state_from_str(&row.state)?,
        outcome: outcome_from_str(&row.outcome)?,
        tentative_outcome: None,
        wins: row.wins,
        losses: row.losses,
        needs_attention: row.needs_attention,
        attention_reason: row.attention_reason.clone(),
        started_at_ms: row.started_at_ms,
        ended_at_ms: row.ended_at_ms,
        revision: row.revision,
        notes: row.notes.clone(),
        game_results,
        manual: row.manual,
        limited: is_limited_format(row.format.as_deref()),
        opponent_colors: None,
        opponent_archetype: row
            .opponent_archetype
            .as_ref()
            .map(OpponentArchetypeView::from_seat),
        rank: row.rank.as_ref().map(MatchRankView::from_rank),
        rating: row.rating.as_ref().map(RatingView::from_rating),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FileView {
    pub path: String,
    #[specta(type = specta_typescript::Number)]
    pub size: u64,
    #[specta(type = specta_typescript::Number)]
    pub cursor: u64,
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_live_growth_ms: Option<i64>,
    pub bad_timezone: bool,
}

/// What the status bar and first-run states show (spec 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    /// The tracker has reported at least once since the app started.
    pub started: bool,
    /// Folders searched for logs (detected or the override).
    pub searched: Vec<String>,
    pub files: Vec<FileView>,
    /// Where Arena's `Player.log` is looked for (detected).
    pub arena_searched: Vec<String>,
    /// Each Arena `Player.log` being read.
    pub arena_files: Vec<FileView>,
    pub two_clients: bool,
    /// Matches held until the local MTGO username is known.
    pub awaiting_identity: u32,
    pub quarantined: u32,
    /// Matches whose last projection failed; a future update retries them.
    pub failed_matches: u32,
    /// Set while the tracker restarts after an error.
    pub restarting: Option<String>,
    /// Set when the OS timezone could not be used (dates fall back to UTC).
    pub timezone_warning: Option<String>,
    /// Offline mode is on (0.x's shared `offlineMode` prop): the status bar
    /// shows the amber pill. Kept by the bridge across tracker reports.
    pub offline_mode: bool,
    /// The watcher is off (release spec 4.1): the status bar says "Stopped".
    pub paused: bool,
}

impl StatusView {
    /// Before the tracker's first report.
    pub fn waiting(timezone_warning: Option<String>) -> Self {
        Self {
            started: false,
            searched: Vec::new(),
            files: Vec::new(),
            arena_searched: Vec::new(),
            arena_files: Vec::new(),
            two_clients: false,
            awaiting_identity: 0,
            quarantined: 0,
            failed_matches: 0,
            restarting: None,
            timezone_warning,
            offline_mode: false,
            paused: false,
        }
    }

    pub fn from_status(status: &tracker::Status) -> Self {
        Self {
            started: true,
            searched: paths(&status.roots),
            files: file_views(&status.files),
            arena_searched: paths(&status.arena_searched),
            arena_files: file_views(&status.arena_files),
            two_clients: status.two_clients,
            awaiting_identity: status.awaiting_identity.len() as u32,
            quarantined: status.quarantined,
            failed_matches: status.failed_matches,
            restarting: None,
            timezone_warning: status.timezone_warning.clone(),
            offline_mode: false,
            paused: status.paused,
        }
    }
}

fn paths(paths: &[std::path::PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

fn file_views(files: &[tracker::FileStatus]) -> Vec<FileView> {
    files
        .iter()
        .map(|file| FileView {
            path: file.path.to_string_lossy().into_owned(),
            size: file.size,
            cursor: file.cursor,
            last_live_growth_ms: file.last_live_growth_ms,
            bad_timezone: file.bad_timezone,
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod testing {
    use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome};

    use super::{MatchSummary, SummaryOutcome, SummaryState};

    fn game(number: u32, result: GameResult) -> GameView {
        GameView {
            mtgo_game_id: Some(100 + u64::from(number)),
            game_number: number,
            result,
            end_reason: EndReason::Win,
            on_play: Some(true),
            local_mulligans: Some(0),
            opp_mulligans: Some(0),
            local_die_roll: None,
            opp_die_roll: None,
            turn_count: Some(8),
            started_at_ms: Some(1_000),
            ended_at_ms: Some(2_000),
        }
    }

    /// A 2-1 match against a fake opponent; complete matches are won.
    pub(crate) fn view(token: &str, started_at_ms: i64, state: MatchState) -> MatchView {
        let complete = state == MatchState::Complete;
        MatchView {
            token: token.to_string(),
            mtgo_match_id: Some(99),
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            league_token: None,
            game_structure: None,
            state,
            resolution: None,
            outcome: if complete {
                Outcome::Win
            } else {
                Outcome::Unknown
            },
            tentative_outcome: if complete { None } else { Some(Outcome::Win) },
            wins: 2,
            losses: 1,
            started_at_ms,
            ended_at_ms: complete.then_some(started_at_ms + 4_000),
            last_activity_ms: started_at_ms + 4_000,
            games: vec![
                game(1, GameResult::Win),
                game(2, GameResult::Loss),
                game(3, GameResult::Win),
            ],
        }
    }

    pub(crate) fn summary(
        token: &str,
        started_at_ms: i64,
        revision: u64,
        state: SummaryState,
    ) -> MatchSummary {
        MatchSummary {
            id: None,
            token: token.to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("Modern".to_string()),
            match_type: Some("Constructed".to_string()),
            platform: crate::platform::PlatformView::Mtgo,
            best_of: None,
            state,
            outcome: SummaryOutcome::Unknown,
            tentative_outcome: None,
            wins: 1,
            losses: 0,
            needs_attention: false,
            attention_reason: None,
            started_at_ms,
            ended_at_ms: None,
            revision,
            notes: None,
            game_results: Vec::new(),
            manual: false,
            limited: false,
            opponent_colors: None,
            opponent_archetype: None,
            rank: None,
            rating: None,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_commander_archetype_view_carries_its_cards_for_the_hover() {
        let seat = |commander_cards| store::match_list::SeatArchetype {
            id: 3,
            name: "Qkraum & Qtymna".to_string(),
            color_identity: Some("U,R,W".to_string()),
            commander_cards,
        };
        assert!(
            OpponentArchetypeView::from_seat(&seat(Vec::new()))
                .commander_cards
                .is_empty()
        );
        let view = OpponentArchetypeView::from_seat(&seat(vec![
            store::match_list::CommanderCard {
                catalog_id: 9001,
                name: Some("Qkraum".to_string()),
                color_identity: Some("R,U".to_string()),
                image: Some("https://cards/9001.jpg".to_string()),
                local_image: None,
            },
            store::match_list::CommanderCard {
                catalog_id: 9002,
                name: None,
                color_identity: None,
                image: None,
                local_image: None,
            },
        ]));
        assert_eq!(
            view.commander_cards,
            vec![
                CommanderCardView {
                    catalog_id: 9001,
                    name: "Qkraum".to_string(),
                    image: Some("https://cards/9001.jpg".to_string()),
                    local_image: None,
                    colors: vec!["U".to_string(), "R".to_string()],
                },
                CommanderCardView {
                    catalog_id: 9002,
                    name: "Card 9002".to_string(),
                    image: None,
                    local_image: None,
                    colors: Vec::new(),
                },
            ]
        );
    }

    use super::testing::view;
    use super::*;
    use fixture_tools::{TestDir, fixture_names, load_fixture, prepared_events};
    use mtgo_match::{MatchState, Projector};
    use mtgo_protocol::EventKind;
    use store::write::{MatchWrite, WriteBatch, commit};
    use store::{open_writer, read};

    // Issue #332: a Limited row offers no "Set deck".
    #[test]
    fn a_draft_or_sealed_format_code_marks_the_row_limited() {
        let modern = view("q-modern", 1_000, MatchState::Complete);
        assert!(!from_view(&modern, 1, Platform::Mtgo, None).limited);
        for code in ["DHOBHOBHOB", "S6FRA"] {
            let draft = MatchView {
                format: Some(code.to_string()),
                ..view("q-draft", 1_000, MatchState::Complete)
            };
            assert!(from_view(&draft, 1, Platform::Mtgo, None).limited, "{code}");
            assert!(
                from_manual_view(&draft, 1, Platform::Mtgo, None).limited,
                "{code}"
            );
        }
        let unknown = MatchView {
            format: None,
            ..view("q-none", 1_000, MatchState::Complete)
        };
        assert!(!from_view(&unknown, 1, Platform::Mtgo, None).limited);
    }

    #[test]
    fn format_codes_become_labels_like_0x() {
        assert_eq!(format_label(Some("CMODERN")).as_deref(), Some("Modern"));
        assert_eq!(format_label(Some("CStandard")).as_deref(), Some("Standard"));
        assert_eq!(
            format_label(Some("Commander")).as_deref(),
            Some("Commander")
        );
        assert_eq!(
            format_label(Some("DHOBHOBHOB")).as_deref(),
            Some("Dhobhobhob")
        );
        assert_eq!(format_label(Some("")), None);
        assert_eq!(format_label(None), None);
    }

    #[test]
    fn format_label_names_arena_formats() {
        for (code, label) in [
            ("CPIONEER", "Pioneer"),
            ("CSTANDARD", "Standard"),
            ("ARENA_HISTORIC", "Historic"),
            ("ARENA_TIMELESS", "Timeless"),
            ("ARENA_ALCHEMY", "Alchemy"),
            ("ARENA_BRAWL", "Brawl"),
            ("ARENA_HISTORIC_BRAWL", "Historic Brawl"),
            ("CMODERN", "Modern"),
        ] {
            assert_eq!(format_label(Some(code)).as_deref(), Some(label), "{code}");
        }
    }

    #[test]
    fn match_type_labels() {
        assert_eq!(match_type_label(Some("ranked")).as_deref(), Some("Ranked"));
        assert_eq!(
            match_type_label(Some("unranked")).as_deref(),
            Some("Unranked")
        );
        assert_eq!(match_type_label(Some("League")).as_deref(), Some("League"));
        assert_eq!(match_type_label(None), None);
    }

    #[test]
    fn an_arena_view_names_its_opponent_and_platform() {
        let mut arena = view("t-arena", 5_000, MatchState::InProgress);
        arena.opponent = Some("arena:u1".to_string());
        arena.match_type = Some("ranked".to_string());
        let summary = from_view(&arena, 1, Platform::Arena, Some("Opponent".to_string()));
        assert_eq!(summary.opponent.as_deref(), Some("Opponent"));
        assert_eq!(summary.platform, PlatformView::Arena);
        assert_eq!(summary.match_type.as_deref(), Some("Ranked"));
        let json = serde_json::to_value(&summary).expect("json");
        assert_eq!(json["platform"], "arena");
    }

    #[test]
    fn a_match_summary_carries_the_arena_kind() {
        let mut arena = view("t-kind", 5_000, MatchState::Complete);
        arena.game_structure = Some("bo1".to_string());
        assert_eq!(
            from_view(&arena, 1, Platform::Arena, None).best_of,
            Some(BestOfView::Bo1)
        );
        arena.game_structure = Some("bo3".to_string());
        assert_eq!(
            from_view(&arena, 1, Platform::Arena, None).best_of,
            Some(BestOfView::Bo3)
        );
        // An MTGO match holds a structure code, never a kind.
        arena.game_structure = Some("Modern".to_string());
        assert_eq!(from_view(&arena, 1, Platform::Mtgo, None).best_of, None);
        assert_eq!(from_view(&arena, 1, Platform::Arena, None).best_of, None);
        let mut row = store::read::MatchRow {
            id: 7,
            token: "t1".to_string(),
            opponent: None,
            format: None,
            match_type: None,
            platform: Platform::Arena,
            league_token: None,
            game_structure: Some("bo3".to_string()),
            state: "complete".to_string(),
            outcome: "win".to_string(),
            wins: 2,
            losses: 0,
            needs_attention: false,
            attention_reason: None,
            started_at_ms: 1,
            ended_at_ms: None,
            revision: 1,
            notes: None,
            manual: false,
            opponent_archetype: None,
            rank: None,
            rating: None,
            games: Vec::new(),
        };
        assert_eq!(from_row(&row).expect("row").best_of, Some(BestOfView::Bo3));
        assert_eq!(from_row(&row).expect("row").rank, None);
        row.rank = Some(store::read::MatchRank {
            class: "Bronze".to_string(),
            level: 2,
            step: 0,
            mythic_percentile: None,
            mythic_placement: None,
        });
        assert_eq!(
            from_row(&row).expect("row").rank,
            Some(MatchRankView {
                class: "Bronze".to_string(),
                level: 2,
                step: 0,
                mythic_percentile: None,
                mythic_placement: None,
                label: "Bronze 2".to_string(),
            })
        );
        row.rank = Some(store::read::MatchRank {
            class: "Mythic".to_string(),
            level: 1,
            step: 0,
            mythic_percentile: Some(85.4),
            mythic_placement: None,
        });
        let mythic = from_row(&row).expect("row").rank.expect("rank");
        assert_eq!(mythic.label, "Mythic 85.4%");
        assert_eq!(mythic.mythic_percentile, Some(85.4));
        row.rank = None;
        row.platform = Platform::Mtgo;
        row.game_structure = Some("Modern".to_string());
        assert_eq!(from_row(&row).expect("row").best_of, None);
    }

    #[test]
    fn a_view_becomes_a_summary_with_a_game_count_score() {
        let mut live = view("t1", 5_000, MatchState::InProgress);
        // The score line says 2-0 but one game has no known winner yet: the row
        // counts games, as the stored row does.
        live.wins = 2;
        live.losses = 0;
        live.games[1].result = mtgo_match::GameResult::Unknown;
        let summary = from_view(&live, 7, Platform::Mtgo, None);
        assert_eq!(summary.state, SummaryState::InProgress);
        assert_eq!(summary.tentative_outcome, Some(SummaryOutcome::Win));
        assert_eq!((summary.wins, summary.losses), (2, 0));
        assert_eq!(summary.format.as_deref(), Some("Modern"));
        assert_eq!(summary.revision, 7);
        assert!(!summary.needs_attention);
        // The game with no winner yet keeps its place: game 3 stays game 3.
        assert_eq!(
            summary.game_results,
            vec![
                GameResultView {
                    won: Some(true),
                    on_play: Some(true)
                },
                GameResultView {
                    won: None,
                    on_play: Some(true)
                },
                GameResultView {
                    won: Some(true),
                    on_play: Some(true)
                },
            ]
        );
        assert_eq!(summary.notes, None);
    }

    #[test]
    fn a_view_leaves_out_its_trailing_games_with_no_winner() {
        let mut live = view("t1", 5_000, MatchState::InProgress);
        live.games[2].result = mtgo_match::GameResult::Unknown;
        let summary = from_view(&live, 7, Platform::Mtgo, None);
        let won: Vec<Option<bool>> = summary.game_results.iter().map(|g| g.won).collect();
        assert_eq!(won, vec![Some(true), Some(false)], "the game being played");
    }

    #[test]
    fn a_stored_row_lists_its_games_in_place_and_its_note() {
        let row = store::read::MatchRow {
            id: 7,
            token: "t1".to_string(),
            opponent: None,
            format: None,
            match_type: None,
            platform: Platform::Mtgo,
            league_token: None,
            game_structure: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            wins: 1,
            losses: 1,
            needs_attention: false,
            attention_reason: None,
            started_at_ms: 1,
            ended_at_ms: None,
            revision: 1,
            notes: Some("Q note".to_string()),
            manual: false,
            opponent_archetype: None,
            rank: None,
            rating: None,
            games: vec![
                store::read::RowGame {
                    result: "loss".to_string(),
                    on_play: Some(false),
                },
                store::read::RowGame {
                    result: "unknown".to_string(),
                    on_play: None,
                },
                store::read::RowGame {
                    result: "win".to_string(),
                    on_play: None,
                },
            ],
        };
        let summary = from_row(&row).expect("shown");
        assert_eq!(summary.id, Some(7));
        assert_eq!(summary.notes.as_deref(), Some("Q note"));
        assert_eq!(
            summary.game_results,
            vec![
                GameResultView {
                    won: Some(false),
                    on_play: Some(false)
                },
                GameResultView {
                    won: None,
                    on_play: None
                },
                GameResultView {
                    won: Some(true),
                    on_play: None
                },
            ],
            "an unknown game keeps its place: Win, Unknown, Loss is G1, G2 ?, G3"
        );
    }

    #[test]
    fn an_unknown_stored_state_is_skipped() {
        let row = store::read::MatchRow {
            id: 7,
            token: "t1".to_string(),
            opponent: None,
            format: None,
            match_type: None,
            platform: Platform::Mtgo,
            league_token: None,
            game_structure: None,
            state: "pending".to_string(),
            outcome: "unknown".to_string(),
            wins: 0,
            losses: 0,
            needs_attention: false,
            attention_reason: None,
            started_at_ms: 1,
            ended_at_ms: None,
            revision: 1,
            notes: None,
            manual: false,
            games: Vec::new(),
            opponent_archetype: None,
            rank: None,
            rating: None,
        };
        assert_eq!(from_row(&row), None);
    }

    #[test]
    fn the_ui_sees_camel_case_fields_and_snake_case_names() {
        let json = serde_json::to_value(from_view(
            &view("t1", 5, MatchState::Complete),
            3,
            Platform::Mtgo,
            None,
        ))
        .expect("json");
        assert_eq!(json["startedAtMs"], 5);
        assert_eq!(json["state"], "complete");
        assert_eq!(json["outcome"], "win");
        assert_eq!(json["tentativeOutcome"], serde_json::Value::Null);
    }

    /// Live equals rebuild, as the UI sees it: every fixture's matches give the
    /// same summary from the projector as from the stored row.
    #[test]
    fn live_and_stored_summaries_agree_on_every_fixture() {
        for name in fixture_names() {
            let fixture = load_fixture(&name);
            let mut projector = Projector::new(vec![]);
            for event in prepared_events(&fixture) {
                if event.kind == EventKind::Login
                    && let Some(user) = &event.local_username
                {
                    projector.add_account(user);
                }
                projector.apply(&event);
            }
            projector.finalize(fixture.meta.now_ms());
            let views: Vec<_> = projector.views().into_iter().cloned().collect();

            let dir = TestDir::new(&format!("summary-{name}"));
            let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
            let batch = WriteBatch {
                matches: views
                    .iter()
                    .map(|view| MatchWrite::Upsert {
                        view: Box::new(view.clone()),
                        revision: projector.revision(&view.token),
                    })
                    .collect(),
                ..WriteBatch::default()
            };
            commit(&mut conn, &batch).expect("commit");

            for row in read::list_matches(&conn, 1_000, 0).expect("list") {
                let view = views
                    .iter()
                    .find(|view| view.token == row.token)
                    .expect("stored row has a view");
                let mut live =
                    from_view(view, projector.revision(&view.token), Platform::Mtgo, None);
                // The tentative outcome is a live hint only (spec 5.5), and
                // the row id comes from storage (the list keeps it).
                live.tentative_outcome = None;
                live.id = u32::try_from(row.id).ok();
                assert_eq!(Some(live), from_row(&row), "{name} {}", row.token);
            }
        }
    }

    #[test]
    fn a_status_becomes_a_status_view() {
        let status = tracker::Status {
            roots: vec![std::path::PathBuf::from(
                "C:/Users/Q/AppData/Local/Apps/2.0",
            )],
            files: vec![tracker::FileStatus {
                path: std::path::PathBuf::from("C:/x/mtgo.log"),
                size: 10,
                cursor: 8,
                last_live_growth_ms: Some(1_234),
                dropped_orphan_lines: 0,
                bad_timezone: false,
            }],
            arena_searched: vec![std::path::PathBuf::from("C:/a/MTGA/Player.log")],
            arena_files: vec![tracker::FileStatus {
                path: std::path::PathBuf::from("C:/a/MTGA/Player.log"),
                size: 20,
                cursor: 20,
                last_live_growth_ms: Some(5_678),
                dropped_orphan_lines: 0,
                bad_timezone: false,
            }],
            two_clients: false,
            awaiting_identity: vec!["t1".to_string(), "t2".to_string()],
            quarantined: 1,
            failed_matches: 2,
            timezone_warning: Some("tz".to_string()),
            paused: true,
        };
        let view = StatusView::from_status(&status);
        assert!(view.started);
        assert!(
            view.paused,
            "the watcher is off: the status bar says Stopped"
        );
        assert_eq!(
            view.searched,
            vec!["C:/Users/Q/AppData/Local/Apps/2.0".to_string()]
        );
        assert_eq!(view.files[0].last_live_growth_ms, Some(1_234));
        assert_eq!(
            view.arena_searched,
            vec!["C:/a/MTGA/Player.log".to_string()]
        );
        assert_eq!(view.arena_files[0].last_live_growth_ms, Some(5_678));
        assert_eq!(view.awaiting_identity, 2);
        assert_eq!(view.quarantined, 1);
        assert_eq!(view.failed_matches, 2);
        assert_eq!(view.restarting, None);
        assert_eq!(view.timezone_warning.as_deref(), Some("tz"));
        assert!(!StatusView::waiting(None).started);
    }

    #[test]
    fn the_timezone_warning_comes_from_the_tracker() {
        let status = tracker::Status {
            timezone_warning: Some("Q zone".to_string()),
            ..tracker::Status::default()
        };
        assert_eq!(
            StatusView::from_status(&status).timezone_warning.as_deref(),
            Some("Q zone")
        );
    }
}
