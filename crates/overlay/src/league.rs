//! The league card's state (overlay spec section 9.1, 0.x `BuildOverlayState`
//! on `release_0.46.0`): which run is on the card, its status, record,
//! progress, live match, deck, art and size. One shape for every surface:
//! the league window, the settings preview and (through `publish`) the
//! hosted OBS page. The JSON field names are 0.x's payload, camelCase.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use leagues::RunState;
use leagues::standing::{PAIRING_GAP_MS, StandingFinish, StandingStage, StandingStatus};
use mtgo_match::{MatchState, Outcome};

use crate::art::{art_mode, window_art};
use crate::event_choice::{Candidate, EventKey, choose_event};
use crate::league_input::{
    LeagueCardInput, LeagueMatchInput, LeagueRunInput, OverlayDeck, TournamentInput,
    TournamentMatchInput,
};
use crate::options::{LeagueDisplay, LeagueTheme, OverlayArtwork, OverlaySize};
use crate::sideboarding::{MatchPhase, match_phase};

/// The stamp 0.x's `MintLeague` (and v1's run naming) adds to a run name:
/// `"{structure} League dd-dd-dddd hh:mm(am|pm)"`, matched case-insensitively
/// at the end. ASCII digits and spaces only, as PHP's non-unicode `\d`, `\s`.
static MINTED_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[ \t\n\x0B\x0C\r]+[0-9]{2}-[0-9]{2}-[0-9]{4} [0-9]{2}:[0-9]{2}[ap]m$")
        .expect("the minted suffix pattern is valid")
});

/// The card's status (0.x `status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LeagueStatus {
    /// No run on the card.
    Idle,
    /// An active run between matches.
    Waiting,
    InGame,
    Sideboarding,
    /// A league run complete, or a tournament finished with no cut.
    Complete,
    /// A league run complete with every round won, or a tournament won.
    Trophied,
    Dropped,
    /// A tournament joined, before round 1 (also after its start time until
    /// round 1 pairs).
    Registered,
    /// A tournament: the account's match is over or the round is between
    /// rounds, and the event goes on.
    WaitingForRound,
    /// A tournament round that is a bye for the account.
    Bye,
    /// A tournament's Swiss rounds are over and the account is in the cut,
    /// before the playoffs.
    MadeCut,
    /// A tournament: lost a playoff match (a top cut finish).
    TopCut,
    /// A tournament's Swiss rounds are over and the account is not in the cut.
    MissedCut,
    /// Removed from a tournament by the event for another reason.
    Eliminated,
}

/// The kind of event on the card (0.x only ever sends `league`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    League,
    /// A Swiss event with or without a cut (Challenges, Preliminaries,
    /// Qualifiers).
    Tournament,
}

/// The card's event block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct LeagueEvent {
    pub kind: EventKind,
    /// The run name, minted date suffix dropped.
    pub name: String,
    /// The format label (`Modern`), empty when unknown.
    pub format: String,
}

/// Wins and losses over the run's complete matches (a tournament's byes
/// count as wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct LeagueRecord {
    pub wins: u32,
    pub losses: u32,
    /// Left out when there are none (a league payload never has it).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub draws: Option<u32>,
}

/// Matches played out of the run's round count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct LeagueProgress {
    pub played: u32,
    pub of: u32,
}

/// One game pip: won, lost, or neither (`null`: live, drawn or unknown).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct GamePip {
    pub won: Option<bool>,
}

/// The live match of an active run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueMatch {
    /// Its place among the run's non-abandoned matches, from 1.
    pub number: u32,
    pub games: Vec<GamePip>,
    pub games_won: u32,
    pub games_lost: u32,
    /// Life while a game is on (`in_game` only).
    pub life: Option<LiveLife>,
    /// The opponent's archetype name as the game overlay shows it; `None`
    /// when unknown (the card then says CHALLENGER), and then left out, so
    /// the 1.3 payload stays as it was. The pure card never knows it: the
    /// app fills it in for the live match.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub opponent_archetype: Option<String>,
}

/// Both players' life in the live game and what they started on (league
/// overlay themes spec 4.2). Read from the game's first and newest frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LiveLife {
    pub you: i32,
    pub opponent: i32,
    pub start_you: i32,
    pub start_opponent: i32,
}

/// Games won and lost across a finished run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct GameRecord {
    pub won: u32,
    pub lost: u32,
}

/// The card's deck block (0.x `BuildOverlayState::deckPayload`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueDeck {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub name: String,
    /// The deck's archetype name; `null` before detection.
    pub archetype: Option<String>,
    /// WUBRG letters, colourless left out.
    pub color_identity: Vec<String>,
    /// The streamer's name for the deck (`decks.overlay_label`).
    pub label: Option<String>,
}

/// The card's background image, or with `thumbnail` the square beside the
/// text (artwork `thumbnail`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct OverlayArt {
    pub url: String,
    pub thumbnail: bool,
}

/// Everything the card shows (spec 9.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueOverlayState {
    pub status: LeagueStatus,
    pub event: Option<LeagueEvent>,
    pub record: Option<LeagueRecord>,
    pub progress: Option<LeagueProgress>,
    #[serde(rename = "match")]
    pub live_match: Option<LeagueMatch>,
    pub game_record: Option<GameRecord>,
    pub deck: Option<LeagueDeck>,
    /// The window's art: a local or remote image, or the custom background.
    pub art: Option<OverlayArt>,
    pub size: OverlaySize,
    /// What the card shows (`overlay_league_display`).
    pub display: LeagueDisplay,
    /// The card's look (`overlay_league_theme`).
    pub theme: LeagueTheme,
    /// Present only on a tournament card.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub tournament: Option<TournamentBlock>,
    /// The other event strip (event overlay spec 6 rule 6), present only
    /// when it shows and the strip switch is on.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[specta(optional)]
    pub other: Option<OtherEvent>,
    /// The card deck's Scryfall art crop, for the public payload's deck art
    /// (`publish`). Never sent to a page.
    #[serde(skip)]
    pub public_deck_art: Option<String>,
}

/// What kind of tournament it is, from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TournamentSeries {
    Challenge,
    ShowcaseChallenge,
    Qualifier,
    Preliminary,
    Other,
}

/// The stage of a tournament round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TournamentStage {
    Swiss,
    /// The quarterfinals of a top 8 cut.
    #[serde(rename = "top8")]
    Top8,
    Semifinal,
    Final,
}

/// A tournament card's own numbers (event overlay spec 7). Unknown values
/// are `null`, never guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct TournamentBlock {
    pub series: TournamentSeries,
    /// The current round, or the last one played once finished. Playoff
    /// rounds continue the count. 1-based.
    pub round: u32,
    /// The stage of `round` (while waiting, the stage just played: the card
    /// names the next one).
    pub stage: TournamentStage,
    /// Swiss rounds; `null` when unknown (the card shows no "/N").
    pub swiss_rounds: Option<u32>,
    /// Players in the top cut; `null` with no cut (a Preliminary).
    pub cut_size: Option<u32>,
    /// The account's rank in the latest round result.
    pub rank: Option<u32>,
    /// Players in the event, from round 1 on.
    pub players: Option<u32>,
    /// Open matches in the current round, every player's. Only while
    /// waiting for the next round.
    pub matches_left: Option<u32>,
    /// Estimated next round start, UTC ms: the round result's log time plus
    /// the pairing gap. Only once the round's result is in.
    #[specta(type = Option<specta_typescript::Number>)]
    pub next_round_at_ms: Option<i64>,
    /// The event's scheduled start, UTC ms. Only while registered.
    #[specta(type = Option<specta_typescript::Number>)]
    pub starts_at_ms: Option<i64>,
    /// Final place, for the finishes that have one.
    pub place: Option<u32>,
}

/// The other event on the strip below the card (event overlay spec 8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
pub struct OtherEvent {
    pub kind: EventKind,
    /// The event name (league run name or tournament name).
    pub name: String,
    pub status: LeagueStatus,
    pub record: Option<LeagueRecord>,
    /// The live match, when there is one.
    #[serde(rename = "match")]
    pub live_match: Option<OtherMatch>,
    /// Present for a tournament.
    pub tournament: Option<TournamentBlock>,
}

/// The other event's live match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OtherMatch {
    /// The game being played or next (sideboarding), from 1.
    pub game: u32,
    pub games_won: u32,
    pub games_lost: u32,
}

/// What the card looks like, from the settings and the app (spec 9.3, 9.4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardLook {
    /// `overlay_size`.
    pub size: OverlaySize,
    /// `overlay_artwork` (unset: the read-time default, `art::art_mode`).
    pub artwork: Option<OverlayArtwork>,
    /// `overlay_league_display`.
    pub display: LeagueDisplay,
    /// `overlay_league_theme`.
    pub theme: LeagueTheme,
    /// The window URL of the custom background, `Some` only when its file
    /// exists (`overlay://localhost/background`, or the Windows form).
    pub background_url: Option<String>,
    /// The URL prefix of a file in v1's card images folder
    /// (`card://localhost/`, or `http://card.localhost/` on Windows), so the
    /// window shows the downloaded art crop when there is one.
    pub card_file_base: String,
    /// The URL prefix of a file in v1's deck covers folder (the `deckcover`
    /// scheme), so the window shows a deck's own cover image.
    pub deck_cover_base: String,
    /// `overlay_event_strip`: the other event strip may show. Off, the card
    /// carries no `other`.
    pub strip: bool,
}

/// How long a tournament stays a candidate after its newest fact (event
/// overlay spec 6, D8): a 50-minute round plus the longest wait seen.
pub const STALE_AFTER_MS: i64 = 90 * 60 * 1000;
/// A registered tournament with no pairing this long after its start time
/// leaves the card (the player unregistered while the app was closed).
pub const UNPAIRED_AFTER_START_MS: i64 = 30 * 60 * 1000;

/// The event card for `now_ms` (event overlay spec 6; 0.x
/// `BuildOverlayState::run` for leagues).
///
/// The candidates: every `active` league run with matches, every
/// `complete` or `dropped` run with matches that ended within `finished_ms`
/// (the `overlay_finished_minutes` setting), and every stored tournament
/// until it goes stale, its finished time passes or it never paired.
/// `choose_event` picks the card's event and the strip's; with none, the
/// idle card with the last played deck. `partial` runs never show. The
/// strip is filled only when `look.strip` is on.
pub fn league_state(
    input: LeagueCardInput,
    now_ms: i64,
    finished_ms: i64,
    look: &CardLook,
) -> LeagueOverlayState {
    let choice = choose_event(&candidates(&input, now_ms, finished_ms));
    let local_username = input.local_username.as_deref();
    let Some(shown) = choice.shown else {
        return payload(LeagueStatus::Idle, input.last_played_deck.as_ref(), look);
    };
    let Some(mut state) = event_card(&input, shown, local_username, look) else {
        return payload(LeagueStatus::Idle, input.last_played_deck.as_ref(), look);
    };
    if look.strip {
        state.other = choice
            .other
            .and_then(|key| event_card(&input, key, None, look))
            .map(|card| other_event(&card));
    }
    state
}

/// The card of one event, `None` when the key names nothing in `input`.
fn event_card(
    input: &LeagueCardInput,
    key: EventKey,
    local_username: Option<&str>,
    look: &CardLook,
) -> Option<LeagueOverlayState> {
    match key {
        EventKey::League(id) => {
            let run = input.runs.iter().find(|run| run.id == id)?;
            Some(run_card(run, local_username, look))
        }
        EventKey::Tournament(id) => {
            let tournament = input
                .tournaments
                .iter()
                .find(|tournament| tournament.mtgo_event_id == id)?;
            Some(tournament_card(tournament, local_username, look))
        }
    }
}

/// A league run's card.
fn run_card(
    run: &LeagueRunInput,
    local_username: Option<&str>,
    look: &CardLook,
) -> LeagueOverlayState {
    let matches = card_matches(run);

    let mut wins = 0;
    let mut losses = 0;
    let mut played = 0;
    for member in &matches {
        if member.state == MatchState::Complete {
            played += 1;
            match member.outcome {
                Outcome::Win => wins += 1,
                Outcome::Loss => losses += 1,
                Outcome::Draw | Outcome::Unknown => {}
            }
        }
    }
    let round_count = run.kind.round_count_stated(run.rounds) as u32;

    let mut state = payload(LeagueStatus::Waiting, run.deck.as_ref(), look);
    state.event = Some(LeagueEvent {
        kind: EventKind::League,
        name: display_name(&run.name),
        format: decks::display_format(run.format.as_deref().unwrap_or("")),
    });
    state.record = Some(LeagueRecord {
        wins,
        losses,
        draws: None,
    });
    state.progress = Some(LeagueProgress {
        played,
        of: round_count,
    });

    if is_finished(run) {
        state.status = if run.state == RunState::Dropped {
            LeagueStatus::Dropped
        } else if wins == round_count && losses == 0 {
            LeagueStatus::Trophied
        } else {
            LeagueStatus::Complete
        };
        state.game_record = Some(game_record(&matches));
        return state;
    }

    let Some((index, live)) = newest_live(&matches) else {
        return state;
    };
    set_live_match(&mut state, live, index as u32 + 1, local_username);
    state
}

/// The card's live match block and its in game or sideboarding status.
fn set_live_match(
    state: &mut LeagueOverlayState,
    live: &LeagueMatchInput,
    number: u32,
    local_username: Option<&str>,
) {
    state.status = match match_phase(&live.sideboarding_events, live.newest_game_start) {
        MatchPhase::Sideboarding => LeagueStatus::Sideboarding,
        MatchPhase::InGame => LeagueStatus::InGame,
    };
    let life = if state.status == LeagueStatus::InGame {
        live_life(live, local_username)
    } else {
        None
    };
    let games: Vec<GamePip> = live.games.iter().map(|won| GamePip { won: *won }).collect();
    state.live_match = Some(LeagueMatch {
        number,
        games_won: live.games.iter().filter(|won| **won == Some(true)).count() as u32,
        games_lost: live.games.iter().filter(|won| **won == Some(false)).count() as u32,
        games,
        life,
        opponent_archetype: None,
    });
}

/// A tournament's card (event overlay spec 5.4 and 7): its status from the
/// stored standing and its live match, the record from its matches plus
/// byes, and its numbers. The deck (D9): the current round's match deck,
/// else the registered deck, else the last played deck of its format.
fn tournament_card(
    tournament: &TournamentInput,
    local_username: Option<&str>,
    look: &CardLook,
) -> LeagueOverlayState {
    let mut members: Vec<&TournamentMatchInput> = tournament
        .matches
        .iter()
        .filter(|played| played.member.state != MatchState::Abandoned)
        .collect();
    members.sort_by_key(|played| played.member.started_at_ms);
    let current = members
        .iter()
        .rev()
        .find(|played| played.round.is_some() && played.round == tournament.round);
    let newest_deck = members.iter().rev().find_map(|played| played.deck.as_ref());
    let deck = current
        .and_then(|played| played.deck.as_ref())
        .or(newest_deck)
        .or(tournament.registered_deck.as_ref())
        .or(tournament.format_deck.as_ref());

    let status = tournament_status(tournament);
    let mut state = payload(status, deck, look);
    state.event = Some(LeagueEvent {
        kind: EventKind::Tournament,
        name: tournament.name.clone().unwrap_or_default(),
        format: decks::display_format(tournament.format.as_deref().unwrap_or("")),
    });

    let mut wins = tournament.byes;
    let mut losses = 0;
    let mut draws = 0;
    for played in &members {
        if played.member.state == MatchState::Complete {
            match played.member.outcome {
                Outcome::Win => wins += 1,
                Outcome::Loss => losses += 1,
                Outcome::Draw => draws += 1,
                Outcome::Unknown => {}
            }
        }
    }
    state.record = Some(LeagueRecord {
        wins,
        losses,
        draws: (draws > 0).then_some(draws),
    });

    let finished = tournament.status == StandingStatus::Finished;
    if finished {
        let matches: Vec<&LeagueMatchInput> = members.iter().map(|played| &played.member).collect();
        state.game_record = Some(game_record(&matches));
    } else if let Some(live) = members
        .iter()
        .rev()
        .find(|played| live_in_round(tournament, played))
    {
        let number = live.round.or(tournament.round).unwrap_or(1);
        set_live_match(&mut state, &live.member, number, local_username);
    }
    state.tournament = Some(tournament_block(tournament, state.status));
    state
}

/// The card status of a stored standing before its live match is read.
fn tournament_status(tournament: &TournamentInput) -> LeagueStatus {
    match tournament.status {
        StandingStatus::Registered => LeagueStatus::Registered,
        // Paired: the live match, when there is one, says in game or
        // sideboarding.
        StandingStatus::Playing => LeagueStatus::InGame,
        StandingStatus::WaitingForRound => LeagueStatus::WaitingForRound,
        StandingStatus::Bye => LeagueStatus::Bye,
        StandingStatus::MadeCut => LeagueStatus::MadeCut,
        // `Unregistered` is never stored and never a candidate; it reads as
        // its finish (none: out of the event).
        StandingStatus::Finished | StandingStatus::Unregistered => match tournament.finish {
            Some(StandingFinish::Trophied) => LeagueStatus::Trophied,
            Some(StandingFinish::TopCut) => LeagueStatus::TopCut,
            Some(StandingFinish::MissedCut) => LeagueStatus::MissedCut,
            Some(StandingFinish::Complete) => LeagueStatus::Complete,
            Some(StandingFinish::Dropped) => LeagueStatus::Dropped,
            Some(StandingFinish::Eliminated) | None => LeagueStatus::Eliminated,
        },
    }
}

/// The tournament block for a card with `status`: each number only in the
/// states that show it (spec 7).
fn tournament_block(tournament: &TournamentInput, status: LeagueStatus) -> TournamentBlock {
    let waiting = status == LeagueStatus::WaitingForRound;
    let estimate_shown = waiting || status == LeagueStatus::MadeCut;
    let placed = matches!(
        status,
        LeagueStatus::Trophied
            | LeagueStatus::TopCut
            | LeagueStatus::MissedCut
            | LeagueStatus::Complete
    );
    TournamentBlock {
        series: tournament_series(tournament.name.as_deref().unwrap_or("")),
        round: tournament.round.unwrap_or(1),
        stage: match tournament.stage {
            StandingStage::Swiss => TournamentStage::Swiss,
            StandingStage::Top8 => TournamentStage::Top8,
            StandingStage::Semifinal => TournamentStage::Semifinal,
            StandingStage::Final => TournamentStage::Final,
        },
        swiss_rounds: tournament.swiss_rounds,
        cut_size: tournament.cut_size,
        rank: tournament.rank,
        players: tournament.players,
        matches_left: if waiting {
            tournament.matches_left
        } else {
            None
        },
        // Only once the round's result is in: while a round runs there is
        // no estimate (spec 5.2).
        next_round_at_ms: if estimate_shown {
            tournament.round_result_at_ms.map(|at| at + PAIRING_GAP_MS)
        } else {
            None
        },
        starts_at_ms: if status == LeagueStatus::Registered {
            tournament.starts_at_ms
        } else {
            None
        },
        place: if placed { tournament.place } else { None },
    }
}

/// The kind of tournament from its name (spec 5.2's event label).
pub fn tournament_series(name: &str) -> TournamentSeries {
    let name = name.to_lowercase();
    if name.contains("qualifier") {
        TournamentSeries::Qualifier
    } else if name.contains("showcase") && name.contains("challenge") {
        TournamentSeries::ShowcaseChallenge
    } else if name.contains("challenge") {
        TournamentSeries::Challenge
    } else if name.contains("prelim") {
        TournamentSeries::Preliminary
    } else {
        TournamentSeries::Other
    }
}

/// The strip's view of another event's card.
fn other_event(card: &LeagueOverlayState) -> OtherEvent {
    let event = card.event.as_ref();
    let live_match = card.live_match.as_ref().map(|live| {
        let decided = live.games.iter().filter(|game| game.won.is_some()).count() as u32;
        OtherMatch {
            // The games include the live one; sideboarding, the next one.
            game: if card.status == LeagueStatus::Sideboarding {
                decided + 1
            } else {
                (live.games.len() as u32).max(1)
            },
            games_won: live.games_won,
            games_lost: live.games_lost,
        }
    });
    OtherEvent {
        kind: event.map_or(EventKind::League, |event| event.kind),
        name: event.map(|event| event.name.clone()).unwrap_or_default(),
        status: card.status,
        record: card.record,
        live_match,
        tournament: card.tournament,
    }
}

/// Every event that can be on the card at `now_ms` (spec 6).
fn candidates(input: &LeagueCardInput, now_ms: i64, finished_ms: i64) -> Vec<Candidate> {
    let mut found = Vec::new();
    for run in &input.runs {
        let finished = match run.state {
            RunState::Active => false,
            RunState::Complete | RunState::Dropped => true,
            _ => continue,
        };
        if run.matches.is_empty() {
            continue;
        }
        if finished && shown_until(run, finished_ms).is_none_or(|until| now_ms >= until) {
            continue;
        }
        found.push(Candidate {
            key: EventKey::League(run.id),
            live: !finished && has_live_match(run),
            finished,
            last_transition_ms: run_transition(run),
        });
    }
    for tournament in &input.tournaments {
        if tournament.status == StandingStatus::Unregistered
            || now_ms >= tournament_leaves_at(tournament, finished_ms)
        {
            continue;
        }
        let finished = tournament.status == StandingStatus::Finished;
        found.push(Candidate {
            key: EventKey::Tournament(tournament.mtgo_event_id),
            live: !finished
                && tournament
                    .matches
                    .iter()
                    .any(|played| live_in_round(tournament, played)),
            finished,
            last_transition_ms: tournament.last_transition_at_ms.max(own_match_transition(
                tournament.matches.iter().map(|played| &played.member),
            )),
        });
    }
    found
}

/// A league run's newest real transition: its start, a match's start or
/// end, and the fold's own completion or drop (never a status the player
/// set).
fn run_transition(run: &LeagueRunInput) -> i64 {
    run.started_at_ms
        .max(own_match_transition(run.matches.iter()))
        .max(run.fold_ended_at_ms.unwrap_or(i64::MIN))
}

/// A tournament match that is live in the standing's current round (or one
/// whose round is not known): a match stuck in progress from an earlier
/// round never makes the event live.
fn live_in_round(tournament: &TournamentInput, played: &TournamentMatchInput) -> bool {
    is_live(&played.member)
        && (played.round.is_none()
            || tournament.round.is_none()
            || played.round == tournament.round)
}

/// The newest start or end of the account's own matches.
fn own_match_transition<'a>(matches: impl Iterator<Item = &'a LeagueMatchInput>) -> i64 {
    let mut newest = i64::MIN;
    for member in matches {
        newest = newest
            .max(member.started_at_ms)
            .max(member.ended_at_ms.unwrap_or(i64::MIN));
    }
    newest
}

/// The first ms a tournament is off the card (spec 6, D8): 90 minutes after
/// its newest fact; a finished one once its finished time has passed. A
/// registered one with a start time stays until 30 minutes after its start
/// with no pairing, however early the player registered (MTGO logs nothing
/// while it waits), so its leave time is the later of the two.
fn tournament_leaves_at(tournament: &TournamentInput, finished_ms: i64) -> i64 {
    let stale = tournament.last_event_at_ms + STALE_AFTER_MS;
    if tournament.status == StandingStatus::Finished {
        let ended = tournament
            .finished_at_ms
            .unwrap_or(tournament.last_transition_at_ms);
        return stale.min(ended + finished_ms + 1);
    }
    if tournament.status == StandingStatus::Registered
        && let Some(starts) = tournament.starts_at_ms
    {
        return stale.max(starts + UNPAIRED_AFTER_START_MS);
    }
    stale
}

/// The id of the live match the card shows, if any: the one whose frames
/// the store reads for the life (`league_state` makes the same choice).
pub fn shown_live_match_id(input: &LeagueCardInput, now_ms: i64, finished_ms: i64) -> Option<i64> {
    shown_live_member(input, now_ms, finished_ms).map(|member| member.id)
}

/// The token of the live match the card shows, if any (the same match as
/// `shown_live_match_id`).
pub fn shown_live_match_token(
    input: &LeagueCardInput,
    now_ms: i64,
    finished_ms: i64,
) -> Option<String> {
    shown_live_member(input, now_ms, finished_ms).map(|member| member.token.clone())
}

fn shown_live_member(
    input: &LeagueCardInput,
    now_ms: i64,
    finished_ms: i64,
) -> Option<&LeagueMatchInput> {
    match choose_event(&candidates(input, now_ms, finished_ms)).shown? {
        EventKey::League(id) => {
            let run = input.runs.iter().find(|run| run.id == id)?;
            if is_finished(run) {
                return None;
            }
            let matches = card_matches(run);
            let (_, live) = newest_live(&matches)?;
            Some(live)
        }
        EventKey::Tournament(id) => {
            let tournament = input
                .tournaments
                .iter()
                .find(|tournament| tournament.mtgo_event_id == id)?;
            if tournament.status == StandingStatus::Finished {
                return None;
            }
            // `tournament_card`'s choice: the newest live match of the round.
            tournament
                .matches
                .iter()
                .filter(|played| live_in_round(tournament, played))
                .max_by_key(|played| played.member.started_at_ms)
                .map(|played| &played.member)
        }
    }
}

/// The run's matches the card counts, oldest first: abandoned matches
/// count for nothing (0.x reads only the others).
fn card_matches(run: &LeagueRunInput) -> Vec<&LeagueMatchInput> {
    let mut matches: Vec<&LeagueMatchInput> = run
        .matches
        .iter()
        .filter(|member| member.state != MatchState::Abandoned)
        .collect();
    matches.sort_by_key(|member| member.started_at_ms);
    matches
}

/// The newest live match of `matches` (oldest first) and its index: an
/// older one stuck in progress never takes over.
fn newest_live<'a>(matches: &[&'a LeagueMatchInput]) -> Option<(usize, &'a LeagueMatchInput)> {
    matches
        .iter()
        .enumerate()
        .rev()
        .find(|(_, member)| is_live(member))
        .map(|(index, member)| (index, *member))
}

/// Whether the run ended (`complete` or `dropped`): its card has no live
/// match.
fn is_finished(run: &LeagueRunInput) -> bool {
    matches!(run.state, RunState::Dropped | RunState::Complete)
}

/// Both players' life from the live game's frames: the local seat is the
/// first frame's player with the local username. `None` when a frame, the
/// username or either seat is missing.
fn live_life(member: &LeagueMatchInput, local_username: Option<&str>) -> Option<LiveLife> {
    let first = game_detail::parse_frame(member.first_frame.as_deref()?)?;
    let newest = game_detail::parse_frame(member.newest_frame.as_deref()?)?;
    let (local, opponent) = game_detail::seats(&first, local_username?)?;
    Some(LiveLife {
        you: newest.player(local)?.life as i32,
        opponent: newest.player(opponent)?.life as i32,
        start_you: first.player(local)?.life as i32,
        start_opponent: first.player(opponent)?.life as i32,
    })
}

/// The first ms a finished run with matches is off the card: its end (the
/// later of completed and dropped) plus `finished_ms`, plus 1, as the card
/// shows it while the end is at or after `now - finished_ms`. `None` for a
/// run the card never shows as finished (active, or no matches).
fn shown_until(run: &LeagueRunInput, finished_ms: i64) -> Option<i64> {
    if !is_finished(run) || run.matches.is_empty() {
        return None;
    }
    let ended = run.completed_at_ms.max(run.dropped_at_ms)?;
    Some(ended + finished_ms + 1)
}

/// When the card next changes with no new data: the first ms a finished
/// run still on it at `now_ms` is off it (its `overlay_finished_minutes`
/// ended), or a tournament leaves it (stale, finished time passed, or never
/// paired), the earliest of them. The overlay worker wakes then; the
/// candidates make the same cuts, so the two cannot drift apart.
pub fn changes_at(input: &LeagueCardInput, now_ms: i64, finished_ms: i64) -> Option<i64> {
    let mut earliest: Option<i64> = None;
    let runs = input
        .runs
        .iter()
        .filter_map(|run| shown_until(run, finished_ms));
    let tournaments = input
        .tournaments
        .iter()
        .map(|tournament| tournament_leaves_at(tournament, finished_ms));
    for at in runs.chain(tournaments) {
        if at > now_ms && earliest.is_none_or(|current| at < current) {
            earliest = Some(at);
        }
    }
    earliest
}

/// The tokens of the live matches of every run and tournament in `input`:
/// their live events (sideboarding) can change the card.
pub fn live_tokens(input: &LeagueCardInput) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    let runs = input.runs.iter().flat_map(|run| run.matches.iter());
    for member in runs {
        if is_live(member) {
            tokens.insert(member.token.clone());
        }
    }
    for tournament in &input.tournaments {
        for played in &tournament.matches {
            if live_in_round(tournament, played) {
                tokens.insert(played.member.token.clone());
            }
        }
    }
    tokens
}

/// Whether a match is live (0.x `Started` or `InProgress`; the store reads
/// a `pending` row as InProgress).
fn is_live(member: &LeagueMatchInput) -> bool {
    member.state == MatchState::InProgress
}

fn has_live_match(run: &LeagueRunInput) -> bool {
    run.matches.iter().any(is_live)
}

/// Games won and lost across the run's non-abandoned matches.
fn game_record(matches: &[&LeagueMatchInput]) -> GameRecord {
    let mut record = GameRecord { won: 0, lost: 0 };
    for member in matches {
        for won in &member.games {
            match won {
                Some(true) => record.won += 1,
                Some(false) => record.lost += 1,
                None => {}
            }
        }
    }
    record
}

/// A card with only its status, deck, art and size (0.x
/// `BuildOverlayState::payload`).
fn payload(
    status: LeagueStatus,
    deck: Option<&OverlayDeck>,
    look: &CardLook,
) -> LeagueOverlayState {
    let mode = art_mode(look.artwork, look.background_url.is_some());
    let deck_art = deck.and_then(|deck| {
        let custom = deck
            .cover_file
            .as_ref()
            .map(|file| format!("{}{}", look.deck_cover_base, file));
        let local = deck
            .cover_local_art_crop
            .as_ref()
            .map(|file| format!("{}{}", look.card_file_base, file));
        custom.or(local).or_else(|| deck.cover_art_crop.clone())
    });
    let thumbnail = mode == OverlayArtwork::Thumbnail;
    let art = window_art(mode, deck_art, look.background_url.clone())
        .filter(|url| !url.is_empty())
        .map(|url| OverlayArt { url, thumbnail });
    LeagueOverlayState {
        status,
        event: None,
        record: None,
        progress: None,
        live_match: None,
        game_record: None,
        deck: deck.map(deck_payload),
        art,
        size: look.size,
        display: look.display,
        theme: look.theme,
        tournament: None,
        other: None,
        public_deck_art: deck.and_then(|deck| deck.cover_art_crop.clone()),
    }
}

/// The run name for the card: 0.x's minted date suffix dropped, a typed name
/// kept as it is (0.x `BuildOverlayState::displayName`).
pub fn display_name(name: &str) -> String {
    MINTED_SUFFIX.replace(name, "").into_owned()
}

/// A stored colour identity as WUBRG letters in that order, colourless and
/// anything else left out (0.x `ColorIdentity::normalize` then the WUBRG
/// filter). Accepts `"U,B"` and `"UB"` alike.
pub fn wubrg(color_identity: Option<&str>) -> Vec<String> {
    let canonical = cards::normalize_colors(color_identity).unwrap_or_default();
    canonical
        .split(',')
        .filter(|letter| !letter.is_empty() && *letter != "C")
        .map(str::to_string)
        .collect()
}

/// The card's deck block for a deck (0.x `BuildOverlayState::deckPayload`),
/// public so the settings preview shows the last played deck the same way.
pub fn deck_payload(deck: &OverlayDeck) -> LeagueDeck {
    LeagueDeck {
        id: deck.id,
        name: deck.name.clone(),
        archetype: deck.archetype.clone(),
        color_identity: wubrg(deck.color_identity.as_deref()),
        label: deck.label.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::league_input::{GRACE_MS, finished_ms};
    use crate::options::LeagueGames;
    use leagues::LeagueKind;

    const NOW: i64 = 1_800_000_000_000;
    const MINUTE: i64 = 60 * 1000;
    const HOUR: i64 = 60 * MINUTE;

    fn deck() -> OverlayDeck {
        OverlayDeck {
            id: 7,
            name: "Mono Green Tron".to_string(),
            archetype: None,
            color_identity: Some("G".to_string()),
            label: None,
            cover_art_crop: Some("https://cards.scryfall.io/art_crop/karn.jpg".to_string()),
            cover_local_art_crop: None,
            cover_file: None,
        }
    }

    fn look() -> CardLook {
        CardLook {
            card_file_base: "card://localhost/".to_string(),
            deck_cover_base: "deckcover://localhost/".to_string(),
            ..CardLook::default()
        }
    }

    fn member(
        id: i64,
        state: MatchState,
        outcome: Outcome,
        started_at_ms: i64,
    ) -> LeagueMatchInput {
        LeagueMatchInput {
            id,
            token: format!("token-{id}"),
            state,
            outcome,
            started_at_ms,
            ended_at_ms: None,
            games: Vec::new(),
            sideboarding_events: Vec::new(),
            newest_game_start: None,
            first_frame: None,
            newest_frame: None,
        }
    }

    /// A run with one complete match per result (a win or a loss), as
    /// 0.x's `leagueWithResults`.
    fn run_with_results(results: &[Outcome]) -> LeagueRunInput {
        let mut matches = Vec::new();
        for (i, result) in results.iter().enumerate() {
            let id = i as i64 + 1;
            matches.push(member(
                id,
                MatchState::Complete,
                *result,
                NOW - (10 - id) * HOUR,
            ));
        }
        LeagueRunInput {
            id: 1,
            kind: LeagueKind::Constructed,
            rounds: None,
            state: RunState::Active,
            name: "Modern League".to_string(),
            format: Some("CMODERN".to_string()),
            started_at_ms: NOW - 11 * HOUR,
            completed_at_ms: None,
            dropped_at_ms: None,
            fold_ended_at_ms: None,
            matches,
            deck: Some(deck()),
        }
    }

    fn live_member(id: i64) -> LeagueMatchInput {
        member(
            id,
            MatchState::InProgress,
            Outcome::Unknown,
            NOW - 10 * MINUTE,
        )
    }

    fn input_of(runs: Vec<LeagueRunInput>) -> LeagueCardInput {
        LeagueCardInput {
            runs,
            ..LeagueCardInput::default()
        }
    }

    fn card(runs: Vec<LeagueRunInput>) -> LeagueOverlayState {
        league_state(
            LeagueCardInput {
                runs,
                ..LeagueCardInput::default()
            },
            NOW,
            GRACE_MS,
            &look(),
        )
    }

    /// `run` ended by the fold at `ended_ms`.
    fn finished(mut run: LeagueRunInput, state: RunState, ended_ms: i64) -> LeagueRunInput {
        run.state = state;
        run.fold_ended_at_ms = Some(ended_ms);
        if state == RunState::Dropped {
            run.dropped_at_ms = Some(ended_ms);
        } else {
            run.completed_at_ms = Some(ended_ms);
        }
        run
    }

    /// The golden league card: two results, a live match in game 2 with
    /// game 1 won, a detected deck with a label.
    fn golden_card() -> LeagueOverlayState {
        let mut run = run_with_results(&[Outcome::Win, Outcome::Loss]);
        let mut live = live_member(3);
        live.games = vec![Some(true), None];
        live.newest_game_start = Some((1, 500));
        run.matches.push(live);
        let deck = run.deck.as_mut().expect("deck");
        deck.archetype = Some("Mono Green Tron".to_string());
        deck.label = Some("Big Mana".to_string());
        card(vec![run])
    }

    /// A league payload is 1.3's (`main`) plus only `display.standing`: no
    /// `tournament`, `other` or `record.draws` key (event overlay spec 7).
    /// The JSON is the 1.3 card code's output for `golden_card`.
    #[test]
    fn a_league_payload_is_the_1_3_payload_plus_display_standing() {
        let json = serde_json::to_value(golden_card()).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "status": "in_game",
                "event": { "kind": "league", "name": "Modern League", "format": "Modern" },
                "record": { "wins": 1, "losses": 1 },
                "progress": { "played": 2, "of": 5 },
                "match": {
                    "number": 3,
                    "games": [{ "won": true }, { "won": null }],
                    "gamesWon": 1,
                    "gamesLost": 0,
                    "life": null
                },
                "gameRecord": null,
                "deck": {
                    "id": 7,
                    "name": "Mono Green Tron",
                    "archetype": "Mono Green Tron",
                    "colorIdentity": ["G"],
                    "label": "Big Mana"
                },
                "art": { "url": "https://cards.scryfall.io/art_crop/karn.jpg", "thumbnail": false },
                "size": "full",
                "display": {
                    "games": "dots", "event": true, "format": true, "title": true,
                    "progress": true, "status": true, "record": true,
                    "standing": false
                },
                "theme": "classic"
            })
        );
    }

    // BuildOverlayStateTest

    #[test]
    fn is_idle_with_no_league_showing_the_last_played_deck_art() {
        let state = league_state(
            LeagueCardInput {
                runs: Vec::new(),
                last_played_deck: Some(deck()),
                ..LeagueCardInput::default()
            },
            NOW,
            GRACE_MS,
            &look(),
        );

        assert_eq!(state.status, LeagueStatus::Idle);
        assert_eq!(state.event, None);
        assert_eq!(state.record, None);
        assert_eq!(
            state.art,
            Some(OverlayArt {
                url: "https://cards.scryfall.io/art_crop/karn.jpg".to_string(),
                thumbnail: false,
            })
        );
        assert_eq!(state.size, OverlaySize::Full);
        assert_eq!(
            state.deck.map(|deck| deck.name),
            Some("Mono Green Tron".to_string())
        );
    }

    #[test]
    fn is_waiting_in_an_active_league_with_no_active_match() {
        let state = card(vec![run_with_results(&[
            Outcome::Win,
            Outcome::Win,
            Outcome::Loss,
            Outcome::Win,
        ])]);

        assert_eq!(state.status, LeagueStatus::Waiting);
        assert_eq!(
            state.event,
            Some(LeagueEvent {
                kind: EventKind::League,
                name: "Modern League".to_string(),
                format: "Modern".to_string(),
            })
        );
        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 3,
                losses: 1,
                draws: None
            })
        );
        assert_eq!(state.progress, Some(LeagueProgress { played: 4, of: 5 }));
        assert_eq!(state.live_match, None);
        let deck = state.deck.expect("deck");
        assert_eq!(deck.name, "Mono Green Tron");
        assert_eq!(deck.color_identity, vec!["G".to_string()]);
    }

    #[test]
    fn is_in_game_with_the_match_number_and_real_games_only() {
        let mut run = run_with_results(&[Outcome::Win, Outcome::Win, Outcome::Loss, Outcome::Win]);
        let mut live = live_member(5);
        live.games = vec![Some(true), None];
        live.newest_game_start = Some((1, 500));
        run.matches.push(live);

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::InGame);
        assert_eq!(
            state.live_match,
            Some(LeagueMatch {
                number: 5,
                games: vec![GamePip { won: Some(true) }, GamePip { won: None }],
                games_won: 1,
                games_lost: 0,
                life: None,
                opponent_archetype: None,
            })
        );
    }

    /// A stored frame line with two players and their life.
    fn frame_line(local: i64, opponent: i64) -> String {
        format!(
            "Twitch Info|Game Play Status Update for Game ID: 9, Match ID: 1, Event ID: 2 \
             {{\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":50,\"HandCount\":7,\"Life\":{local}}},\
             {{\"Id\":0,\"Name\":\"Qrival\",\"LibraryCount\":50,\"HandCount\":7,\"Life\":{opponent}}}],\"Cards\":[]}}"
        )
    }

    fn in_game_with_frames(
        first: Option<String>,
        newest: Option<String>,
        local: Option<&str>,
    ) -> LeagueOverlayState {
        let mut run = run_with_results(&[Outcome::Win]);
        let mut live = live_member(2);
        live.games = vec![None];
        live.first_frame = first;
        live.newest_frame = newest;
        run.matches.push(live);
        let input = LeagueCardInput {
            runs: vec![run],
            local_username: local.map(str::to_string),
            ..LeagueCardInput::default()
        };
        league_state(input, NOW, GRACE_MS, &look())
    }

    #[test]
    fn an_in_game_card_carries_both_players_life() {
        let card = in_game_with_frames(
            Some(frame_line(20, 20)),
            Some(frame_line(14, 4)),
            Some("Qlocal"),
        );
        assert_eq!(card.status, LeagueStatus::InGame);
        let life = card.live_match.expect("live").life.expect("life");
        assert_eq!(
            life,
            LiveLife {
                you: 14,
                opponent: 4,
                start_you: 20,
                start_opponent: 20
            }
        );
        let json = serde_json::to_value(life).expect("json");
        assert_eq!(
            json,
            serde_json::json!({"you": 14, "opponent": 4, "startYou": 20, "startOpponent": 20})
        );
    }

    #[test]
    fn life_is_none_without_frames_or_the_local_seat() {
        let no_frames = in_game_with_frames(None, None, Some("Qlocal"));
        assert_eq!(no_frames.live_match.expect("live").life, None);
        let stranger = in_game_with_frames(
            Some(frame_line(20, 20)),
            Some(frame_line(14, 4)),
            Some("Qnobody"),
        );
        assert_eq!(stranger.live_match.expect("live").life, None);
        let no_name = in_game_with_frames(Some(frame_line(20, 20)), Some(frame_line(14, 4)), None);
        assert_eq!(no_name.live_match.expect("live").life, None);
    }

    #[test]
    fn life_is_none_while_sideboarding() {
        let mut run = run_with_results(&[Outcome::Win]);
        let mut live = live_member(2);
        live.games = vec![Some(true)];
        live.newest_game_start = Some((1, 500));
        live.sideboarding_events = vec![(
            (1, 900),
            "Match State Changed from LeagueMatchJoinedGameStartedState to LeagueMatchJoinedSideboardingState"
                .to_string(),
        )];
        live.first_frame = Some(frame_line(20, 20));
        live.newest_frame = Some(frame_line(14, 4));
        run.matches.push(live);
        let input = LeagueCardInput {
            runs: vec![run],
            local_username: Some("Qlocal".to_string()),
            ..LeagueCardInput::default()
        };
        let card = league_state(input, NOW, GRACE_MS, &look());
        assert_eq!(card.status, LeagueStatus::Sideboarding);
        assert_eq!(card.live_match.expect("live").life, None);
    }

    #[test]
    fn is_sideboarding_via_the_log_floor() {
        let mut run = run_with_results(&[Outcome::Win]);
        let mut live = live_member(2);
        live.games = vec![Some(true)];
        live.newest_game_start = Some((1, 500));
        live.sideboarding_events = vec![(
            (1, 900),
            "Match State Changed from LeagueMatchJoinedGameStartedState to LeagueMatchJoinedSideboardingState"
                .to_string(),
        )];
        run.matches.push(live);

        assert_eq!(card(vec![run]).status, LeagueStatus::Sideboarding);
    }

    #[test]
    fn counts_a_null_won_game_towards_neither_side() {
        let mut run = run_with_results(&[]);
        let mut live = live_member(1);
        live.games = vec![Some(true), None, Some(false)];
        run.matches.push(live);

        let live_match = card(vec![run]).live_match.expect("live match");
        assert_eq!(live_match.games_won, 1);
        assert_eq!(live_match.games_lost, 1);
    }

    #[test]
    fn numbers_the_active_match_excluding_abandoned_matches() {
        let mut run = run_with_results(&[Outcome::Win]);
        run.matches.push(member(
            2,
            MatchState::Abandoned,
            Outcome::Unknown,
            NOW - HOUR,
        ));
        run.matches.push(live_member(3));

        assert_eq!(card(vec![run]).live_match.expect("live").number, 2);
    }

    #[test]
    fn is_complete_with_the_game_record_inside_the_grace_window() {
        let mut run = run_with_results(&[
            Outcome::Win,
            Outcome::Win,
            Outcome::Win,
            Outcome::Loss,
            Outcome::Win,
        ]);
        for member in &mut run.matches {
            member.games = vec![Some(true), Some(member.outcome == Outcome::Win)];
        }
        let run = finished(run, RunState::Complete, NOW - MINUTE);

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::Complete);
        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 4,
                losses: 1,
                draws: None
            })
        );
        assert_eq!(state.game_record, Some(GameRecord { won: 9, lost: 1 }));
    }

    #[test]
    fn is_trophied_on_a_perfect_constructed_run() {
        let run = finished(
            run_with_results(&[
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
            ]),
            RunState::Complete,
            NOW - MINUTE,
        );

        assert_eq!(card(vec![run]).status, LeagueStatus::Trophied);
    }

    #[test]
    fn is_trophied_on_a_3_0_draft() {
        let mut run = run_with_results(&[Outcome::Win, Outcome::Win, Outcome::Win]);
        run.kind = LeagueKind::Draft;
        let run = finished(run, RunState::Complete, NOW - MINUTE);

        assert_eq!(card(vec![run]).status, LeagueStatus::Trophied);
    }

    #[test]
    fn is_dropped_with_progress_inside_the_grace_window() {
        let run = finished(
            run_with_results(&[Outcome::Win, Outcome::Loss, Outcome::Loss]),
            RunState::Dropped,
            NOW - MINUTE,
        );

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::Dropped);
        assert_eq!(state.progress, Some(LeagueProgress { played: 3, of: 5 }));
    }

    #[test]
    fn is_idle_once_the_grace_window_has_passed() {
        let run = finished(
            run_with_results(&[
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
                Outcome::Win,
            ]),
            RunState::Complete,
            NOW - 6 * MINUTE,
        );

        assert_eq!(card(vec![run]).status, LeagueStatus::Idle);
    }

    #[test]
    fn normalises_colour_identity_into_wubrg_order_without_colourless() {
        let mut run = run_with_results(&[Outcome::Win]);
        let mut grixis = deck();
        grixis.name = "Grixis".to_string();
        grixis.color_identity = Some("R,C,U,B".to_string());
        run.deck = Some(grixis);

        let deck = card(vec![run]).deck.expect("deck");
        assert_eq!(deck.color_identity, vec!["U", "B", "R"]);
    }

    #[test]
    fn returns_the_compact_size_when_set() {
        let compact = CardLook {
            size: OverlaySize::Compact,
            ..look()
        };

        let state = league_state(LeagueCardInput::default(), NOW, GRACE_MS, &compact);

        assert_eq!(state.size, OverlaySize::Compact);
    }

    #[test]
    fn is_idempotent() {
        let mut run = run_with_results(&[Outcome::Win, Outcome::Loss]);
        run.matches.push(live_member(3));
        let input = LeagueCardInput {
            runs: vec![run],
            last_played_deck: Some(deck()),
            ..LeagueCardInput::default()
        };

        assert_eq!(
            league_state(input.clone(), NOW, GRACE_MS, &look()),
            league_state(input, NOW, GRACE_MS, &look())
        );
    }

    // "stays within a query budget while in game" is U4's
    // `store::overlay` read count test (7 queries whatever the size).

    #[test]
    fn drops_the_minted_date_suffix_from_the_league_name() {
        let mut run = run_with_results(&[Outcome::Win]);
        run.name = "Modern League 14-09-2026 11:01am".to_string();

        assert_eq!(card(vec![run]).event.expect("event").name, "Modern League");
    }

    #[test]
    fn keeps_a_user_chosen_league_name_as_is() {
        let mut run = run_with_results(&[Outcome::Win]);
        run.name = "Friday Night 5-0 Hunt".to_string();

        assert_eq!(
            card(vec![run]).event.expect("event").name,
            "Friday Night 5-0 Hunt"
        );
    }

    #[test]
    fn shows_the_newest_active_match_when_an_older_one_is_stuck_in_progress() {
        let mut run = run_with_results(&[Outcome::Win]);
        run.matches.push(member(
            2,
            MatchState::InProgress,
            Outcome::Unknown,
            NOW - 2 * HOUR,
        ));
        let mut live = live_member(3);
        live.games = vec![Some(true)];
        run.matches.push(live);

        let live_match = card(vec![run]).live_match.expect("live");
        assert_eq!(live_match.number, 3);
        assert_eq!(live_match.games, vec![GamePip { won: Some(true) }]);
    }

    /// Two active runs, each with a live match: the older run's match is the
    /// newer one, so its start is the newest real transition and the card
    /// shows it (event overlay spec 6).
    fn two_live_runs() -> Vec<LeagueRunInput> {
        let mut newer_run = run_with_results(&[Outcome::Win]);
        newer_run.matches.push(live_member(2));
        let mut older_run = run_with_results(&[]);
        older_run.id = 2;
        older_run.started_at_ms = NOW - 12 * HOUR;
        older_run.matches.push(member(
            3,
            MatchState::InProgress,
            Outcome::Unknown,
            NOW - MINUTE,
        ));
        vec![newer_run, older_run]
    }

    #[test]
    fn the_shown_live_match_is_the_cards_live_match() {
        let runs = two_live_runs();
        assert_eq!(
            shown_live_match_id(&input_of(runs.clone()), NOW, GRACE_MS),
            Some(3)
        );
        let card = card(runs);
        assert_eq!(
            card.live_match.expect("live").number,
            1,
            "run 2's match, its first"
        );
    }

    #[test]
    fn the_shown_live_match_token_is_the_shown_matchs_token() {
        let runs = two_live_runs();
        assert_eq!(
            shown_live_match_token(&input_of(runs), NOW, GRACE_MS),
            Some("token-3".to_string())
        );
        assert_eq!(
            shown_live_match_token(&input_of(Vec::new()), NOW, GRACE_MS),
            None
        );
    }

    #[test]
    fn no_match_is_shown_live_for_a_finished_run_or_none() {
        let finished_run = finished(
            run_with_results(&[Outcome::Win, Outcome::Win]),
            RunState::Complete,
            NOW,
        );
        assert_eq!(
            shown_live_match_id(&input_of(vec![finished_run]), NOW, GRACE_MS),
            None
        );
        assert_eq!(
            shown_live_match_id(&input_of(Vec::new()), NOW, GRACE_MS),
            None
        );
        let mut abandoned = run_with_results(&[Outcome::Win]);
        abandoned.matches.push(member(
            2,
            MatchState::Abandoned,
            Outcome::Unknown,
            NOW - MINUTE,
        ));
        assert_eq!(
            shown_live_match_id(&input_of(vec![abandoned]), NOW, GRACE_MS),
            None
        );
    }

    fn card_at(runs: Vec<LeagueRunInput>, now_ms: i64) -> LeagueOverlayState {
        let input = LeagueCardInput {
            runs,
            last_played_deck: None,
            local_username: None,
            tournaments: Vec::new(),
        };
        league_state(input, now_ms, GRACE_MS, &look())
    }

    #[test]
    fn a_finished_run_leaves_the_card_exactly_when_its_grace_ends() {
        let run = finished(
            run_with_results(&[Outcome::Win, Outcome::Loss]),
            RunState::Complete,
            NOW,
        );
        let runs = vec![run];
        let ends =
            changes_at(&input_of(runs.clone()), NOW, GRACE_MS).expect("a finished run on the card");
        assert_eq!(ends, NOW + GRACE_MS + 1);
        assert_eq!(
            card_at(runs.clone(), ends - 1).status,
            LeagueStatus::Complete,
            "still on the card at its last ms"
        );
        assert_eq!(card_at(runs.clone(), ends).status, LeagueStatus::Idle);
        assert_eq!(
            changes_at(&input_of(runs.clone()), ends, GRACE_MS),
            None,
            "nothing ahead"
        );
    }

    #[test]
    fn changes_at_takes_the_earliest_finished_run_still_on_the_card() {
        let mut dropped = finished(
            run_with_results(&[Outcome::Loss]),
            RunState::Dropped,
            NOW - MINUTE,
        );
        dropped.id = 2;
        let complete = finished(run_with_results(&[Outcome::Win]), RunState::Complete, NOW);
        let mut empty = finished(run_with_results(&[]), RunState::Complete, NOW - 2 * MINUTE);
        empty.id = 3;
        let active = run_with_results(&[Outcome::Win]);
        let runs = vec![complete, dropped, empty, active];
        assert_eq!(
            changes_at(&input_of(runs.clone()), NOW, GRACE_MS),
            Some(NOW - MINUTE + GRACE_MS + 1),
            "the dropped run; a run with no matches is never on the card"
        );
    }

    #[test]
    fn live_tokens_are_the_in_progress_matches_of_every_run() {
        let runs = two_live_runs();
        let tokens: Vec<String> = live_tokens(&input_of(runs.clone())).into_iter().collect();
        assert_eq!(tokens, vec!["token-2".to_string(), "token-3".to_string()]);
        assert!(live_tokens(&input_of(vec![run_with_results(&[Outcome::Win])])).is_empty());
    }

    #[test]
    fn includes_the_stored_deck_label() {
        let mut run = run_with_results(&[Outcome::Win]);
        if let Some(deck) = run.deck.as_mut() {
            deck.label = Some("Big Mana".to_string());
        }

        assert_eq!(
            card(vec![run]).deck.expect("deck").label,
            Some("Big Mana".to_string())
        );
    }

    #[test]
    fn sends_a_null_label_when_none_is_stored() {
        let run = run_with_results(&[Outcome::Win]);

        assert_eq!(card(vec![run]).deck.expect("deck").label, None);
    }

    #[test]
    fn sends_the_deck_archetype_for_the_card_title() {
        let mut run = run_with_results(&[Outcome::Win]);
        if let Some(deck) = run.deck.as_mut() {
            deck.archetype = Some("Mono Green Tron".to_string());
        }

        assert_eq!(
            card(vec![run]).deck.expect("deck").archetype,
            Some("Mono Green Tron".to_string())
        );
    }

    #[test]
    fn sends_a_null_archetype_before_detection() {
        let run = run_with_results(&[Outcome::Win]);

        assert_eq!(card(vec![run]).deck.expect("deck").archetype, None);
    }

    // OverlayControllerTest (league), the selection cases.

    fn named(
        name: &str,
        state: RunState,
        started_at_ms: i64,
        members: Vec<LeagueMatchInput>,
    ) -> LeagueRunInput {
        LeagueRunInput {
            id: started_at_ms,
            kind: LeagueKind::Constructed,
            rounds: None,
            state,
            name: name.to_string(),
            format: Some("Modern".to_string()),
            started_at_ms,
            completed_at_ms: None,
            dropped_at_ms: None,
            fold_ended_at_ms: None,
            matches: members,
            deck: None,
        }
    }

    #[test]
    fn renders_overlay_with_no_active_league() {
        let state = card(Vec::new());

        assert_eq!(state.status, LeagueStatus::Idle);
        assert_eq!(state.event, None);
    }

    #[test]
    fn renders_overlay_with_active_league_data() {
        let run = named(
            "Test League",
            RunState::Active,
            NOW,
            vec![
                member(1, MatchState::Complete, Outcome::Win, NOW),
                member(2, MatchState::Complete, Outcome::Loss, NOW),
            ],
        );

        let state = card(vec![run]);

        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 1,
                losses: 1,
                draws: None
            })
        );
        assert_eq!(state.progress.expect("progress").played, 2);
        assert_eq!(state.event.expect("event").format, "Modern");
        assert_eq!(state.status, LeagueStatus::Waiting);
    }

    #[test]
    fn detects_an_active_match_in_the_league() {
        let run = named(
            "Active Match League",
            RunState::Active,
            NOW,
            vec![member(1, MatchState::InProgress, Outcome::Unknown, NOW)],
        );

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::InGame);
        assert_eq!(state.progress.expect("progress").played, 0);
    }

    #[test]
    fn includes_game_results_for_the_active_match() {
        let mut live = member(1, MatchState::InProgress, Outcome::Unknown, NOW);
        live.games = vec![Some(true), None];
        let run = named("Game Results League", RunState::Active, NOW, vec![live]);

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::InGame);
        let games = state.live_match.expect("live").games;
        assert_eq!(
            games,
            vec![GamePip { won: Some(true) }, GamePip { won: None }]
        );
    }

    #[test]
    fn falls_back_to_most_recent_completed_league_when_no_active_league_exists() {
        let members = (1..=5)
            .map(|id| member(id, MatchState::Complete, Outcome::Win, NOW))
            .collect();
        let mut run = named("Completed League", RunState::Complete, NOW, members);
        run.completed_at_ms = Some(NOW);

        let state = card(vec![run]);

        assert_eq!(state.event.expect("event").name, "Completed League");
        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 5,
                losses: 0,
                draws: None
            })
        );
        assert_eq!(state.status, LeagueStatus::Trophied);
    }

    #[test]
    fn hides_completed_league_when_completed_at_is_older_than_5_minutes() {
        let mut run = named(
            "Stale League",
            RunState::Complete,
            NOW - 2 * HOUR,
            vec![member(
                1,
                MatchState::Complete,
                Outcome::Win,
                NOW - 2 * HOUR,
            )],
        );
        run.completed_at_ms = Some(NOW - 10 * MINUTE);

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::Idle);
        assert_eq!(state.event, None);
    }

    #[test]
    fn shows_completed_league_when_completed_at_is_within_5_minutes() {
        let mut run = named(
            "Fresh League",
            RunState::Complete,
            NOW - HOUR,
            vec![member(
                1,
                MatchState::Complete,
                Outcome::Win,
                NOW - 5 * MINUTE,
            )],
        );
        run.completed_at_ms = Some(NOW - 2 * MINUTE);

        let state = card(vec![run]);

        assert_eq!(state.event.expect("event").name, "Fresh League");
        assert_eq!(state.status, LeagueStatus::Complete);
    }

    #[test]
    fn shows_dropped_league_when_dropped_at_is_within_5_minutes() {
        let mut run = named(
            "Fresh Dropped League",
            RunState::Dropped,
            NOW - HOUR,
            vec![member(
                1,
                MatchState::Complete,
                Outcome::Loss,
                NOW - 10 * MINUTE,
            )],
        );
        run.dropped_at_ms = Some(NOW - 2 * MINUTE);

        let state = card(vec![run]);

        assert_eq!(state.event.expect("event").name, "Fresh Dropped League");
        assert_eq!(state.status, LeagueStatus::Dropped);
    }

    #[test]
    fn hides_dropped_league_when_dropped_at_is_older_than_5_minutes() {
        let mut run = named(
            "Stale Dropped League",
            RunState::Dropped,
            NOW - 2 * HOUR,
            vec![member(
                1,
                MatchState::Complete,
                Outcome::Loss,
                NOW - 2 * HOUR,
            )],
        );
        run.dropped_at_ms = Some(NOW - 10 * MINUTE);

        let state = card(vec![run]);

        assert_eq!(state.status, LeagueStatus::Idle);
        assert_eq!(state.event, None);
    }

    /// Event overlay spec 6 rule 5 (replacing 0.x's "an active run beats a
    /// finished one"): a run that just finished shows its result for the
    /// finished time, unless the other run moves later or goes live.
    #[test]
    fn a_just_finished_run_keeps_the_card_until_the_other_run_moves() {
        let mut old = named(
            "Old League",
            RunState::Complete,
            NOW - 24 * HOUR,
            vec![member(
                1,
                MatchState::Complete,
                Outcome::Win,
                NOW - 24 * HOUR,
            )],
        );
        old.completed_at_ms = Some(NOW - MINUTE);
        old.fold_ended_at_ms = Some(NOW - MINUTE);
        let waiting = named(
            "New League",
            RunState::Active,
            NOW - HOUR,
            vec![member(2, MatchState::Complete, Outcome::Loss, NOW - HOUR)],
        );
        let state = card(vec![old.clone(), waiting.clone()]);
        assert_eq!(state.event.expect("event").name, "Old League");
        assert_eq!(state.status, LeagueStatus::Complete);

        let mut moved = waiting;
        moved
            .matches
            .push(member(3, MatchState::Complete, Outcome::Win, NOW));
        let state = card(vec![old, moved]);
        assert_eq!(state.event.expect("event").name, "New League");
        assert_eq!(state.status, LeagueStatus::Waiting);
    }

    /// A run the player marked Complete just now (its shown end is the
    /// edit) is no real transition: the active run keeps the card.
    #[test]
    fn a_run_marked_complete_by_hand_does_not_take_the_card() {
        let mut marked = named(
            "Old League",
            RunState::Complete,
            NOW - 24 * HOUR,
            vec![member(
                1,
                MatchState::Complete,
                Outcome::Win,
                NOW - 24 * HOUR,
            )],
        );
        marked.completed_at_ms = Some(NOW - MINUTE);
        let waiting = named(
            "New League",
            RunState::Active,
            NOW - HOUR,
            vec![member(2, MatchState::Complete, Outcome::Loss, NOW - HOUR)],
        );
        let state = card(vec![marked, waiting]);
        assert_eq!(state.event.expect("event").name, "New League");
    }

    // v1 rules beyond the ported cases.

    #[test]
    fn prefers_an_active_run_with_a_live_match_over_a_newer_one_without() {
        let live = named(
            "Live League",
            RunState::Active,
            NOW - HOUR,
            vec![live_member(1)],
        );
        let newer = named(
            "Newer League",
            RunState::Active,
            NOW,
            vec![member(2, MatchState::Complete, Outcome::Win, NOW)],
        );

        let state = card(vec![newer, live]);

        assert_eq!(state.event.expect("event").name, "Live League");
        assert_eq!(state.status, LeagueStatus::InGame);
    }

    #[test]
    fn a_partial_run_or_a_run_without_matches_is_never_on_the_card() {
        let partial = named(
            "Partial League",
            RunState::Partial,
            NOW,
            vec![member(1, MatchState::Complete, Outcome::Win, NOW)],
        );
        let empty = named("Empty League", RunState::Active, NOW, Vec::new());

        assert_eq!(card(vec![partial, empty]).status, LeagueStatus::Idle);
    }

    #[test]
    fn the_grace_window_includes_its_last_millisecond() {
        let run = finished(
            run_with_results(&[Outcome::Win]),
            RunState::Complete,
            NOW - GRACE_MS,
        );

        assert_eq!(card(vec![run]).status, LeagueStatus::Complete);
    }

    #[test]
    fn the_chosen_finished_time_takes_the_run_off_the_card() {
        let one_minute = finished_ms(1);
        let input = |ended_ms| LeagueCardInput {
            runs: vec![finished(
                run_with_results(&[Outcome::Win]),
                RunState::Complete,
                ended_ms,
            )],
            last_played_deck: None,
            local_username: None,
            tournaments: Vec::new(),
        };

        let shown = league_state(input(NOW - one_minute), NOW, one_minute, &look());
        let gone = league_state(input(NOW - one_minute - 1), NOW, one_minute, &look());

        assert_eq!(shown.status, LeagueStatus::Complete);
        assert_eq!(gone.status, LeagueStatus::Idle);
    }

    #[test]
    fn the_finished_time_is_clamped_to_one_to_sixty_minutes() {
        assert_eq!(finished_ms(0), 60_000);
        assert_eq!(finished_ms(2), 2 * 60_000);
        assert_eq!(finished_ms(5), GRACE_MS);
        assert_eq!(finished_ms(600), 60 * 60_000);
    }

    #[test]
    fn the_window_art_prefers_the_decks_own_cover_but_the_public_art_stays_the_printing() {
        let mut stored = deck();
        stored.cover_local_art_crop = Some("1001_art.jpg".to_string());
        stored.cover_file = Some("7-1700.png".to_string());
        let state = league_state(
            LeagueCardInput {
                runs: Vec::new(),
                last_played_deck: Some(stored),
                ..LeagueCardInput::default()
            },
            NOW,
            GRACE_MS,
            &look(),
        );

        assert_eq!(
            state.art,
            Some(OverlayArt {
                url: "deckcover://localhost/7-1700.png".to_string(),
                thumbnail: false,
            })
        );
        assert_eq!(
            state.public_deck_art.as_deref(),
            Some("https://cards.scryfall.io/art_crop/karn.jpg")
        );
    }

    #[test]
    fn the_window_art_prefers_the_downloaded_art_crop() {
        let mut stored = deck();
        stored.cover_local_art_crop = Some("1001_art.jpg".to_string());
        let state = league_state(
            LeagueCardInput {
                runs: Vec::new(),
                last_played_deck: Some(stored),
                ..LeagueCardInput::default()
            },
            NOW,
            GRACE_MS,
            &look(),
        );

        assert_eq!(
            state.art,
            Some(OverlayArt {
                url: "card://localhost/1001_art.jpg".to_string(),
                thumbnail: false,
            })
        );
        assert_eq!(
            state.public_deck_art.as_deref(),
            Some("https://cards.scryfall.io/art_crop/karn.jpg")
        );
    }

    #[test]
    fn the_custom_background_wins_when_it_exists() {
        let custom = CardLook {
            background_url: Some("overlay://localhost/background".to_string()),
            ..look()
        };
        let state = league_state(
            LeagueCardInput {
                runs: Vec::new(),
                last_played_deck: Some(deck()),
                ..LeagueCardInput::default()
            },
            NOW,
            GRACE_MS,
            &custom,
        );

        assert_eq!(
            state.art,
            Some(OverlayArt {
                url: "overlay://localhost/background".to_string(),
                thumbnail: false,
            })
        );
    }

    #[test]
    fn the_json_matches_0x_payload_names() {
        let mut run = run_with_results(&[Outcome::Win]);
        run.matches.push(live_member(2));
        let json = serde_json::to_value(card(vec![run])).expect("json");

        let keys: Vec<&str> = json
            .as_object()
            .expect("object")
            .keys()
            .map(|key| key.as_str())
            .collect();
        let mut expected = vec![
            "status",
            "event",
            "record",
            "progress",
            "match",
            "gameRecord",
            "deck",
            "art",
            "size",
            "display",
            "theme",
        ];
        expected.sort_unstable();
        let mut keys = keys;
        keys.sort_unstable();
        assert_eq!(keys, expected);
        assert_eq!(json["status"], "in_game");
        assert_eq!(json["event"]["kind"], "league");
        assert_eq!(json["match"]["gamesWon"], 0);
        assert_eq!(json["deck"]["colorIdentity"][0], "G");
        assert_eq!(json["size"], "full");
        assert_eq!(json["theme"], "classic");
        assert_eq!(json["display"]["games"], "dots");
        assert_eq!(json["display"]["record"], true);
    }

    #[test]
    fn display_name_and_wubrg() {
        assert_eq!(
            display_name("Modern League 01-02-2026 09:30PM"),
            "Modern League"
        );
        assert_eq!(display_name("Modern League"), "Modern League");
        assert_eq!(wubrg(Some("gw")), vec!["W", "G"]);
        assert_eq!(wubrg(Some("C")), Vec::<String>::new());
        assert_eq!(wubrg(None), Vec::<String>::new());
    }

    #[test]
    fn every_card_carries_the_theme() {
        let versus = CardLook {
            theme: LeagueTheme::Versus,
            ..look()
        };
        let idle = league_state(LeagueCardInput::default(), NOW, GRACE_MS, &versus);
        assert_eq!(idle.theme, LeagueTheme::Versus);
        let mut run = run_with_results(&[Outcome::Win]);
        run.matches.push(live_member(2));
        let input = LeagueCardInput {
            runs: vec![run],
            ..LeagueCardInput::default()
        };
        assert_eq!(
            league_state(input, NOW, GRACE_MS, &versus).theme,
            LeagueTheme::Versus
        );
    }

    #[test]
    fn every_card_carries_the_display_choices() {
        let display = LeagueDisplay {
            games: LeagueGames::Letters,
            record: false,
            ..LeagueDisplay::default()
        };
        let chosen = CardLook { display, ..look() };
        let idle = league_state(LeagueCardInput::default(), NOW, GRACE_MS, &chosen);
        assert_eq!(idle.display, display);
        let mut run = run_with_results(&[Outcome::Win]);
        run.matches.push(live_member(2));
        let live = league_state(
            LeagueCardInput {
                runs: vec![run],
                last_played_deck: None,
                ..LeagueCardInput::default()
            },
            NOW,
            GRACE_MS,
            &chosen,
        );
        assert_eq!(live.display, display);
        assert_eq!(CardLook::default().display, LeagueDisplay::default());
    }

    #[test]
    fn the_thumbnail_artwork_marks_the_deck_art() {
        let thumbnail = CardLook {
            artwork: Some(OverlayArtwork::Thumbnail),
            ..look()
        };
        let input = || LeagueCardInput {
            runs: vec![run_with_results(&[Outcome::Win])],
            last_played_deck: None,
            ..LeagueCardInput::default()
        };
        let state = league_state(input(), NOW, GRACE_MS, &thumbnail);
        assert!(state.art.expect("the deck art").thumbnail);
        let deck = league_state(input(), NOW, GRACE_MS, &look());
        assert!(!deck.art.expect("the deck art").thumbnail);
    }

    // Tournaments (event overlay spec 5.4, 6 and 7)

    const EVENT_ID: i64 = 12_858_947;

    fn tournament(status: StandingStatus) -> TournamentInput {
        TournamentInput {
            mtgo_event_id: EVENT_ID,
            name: Some("Modern Challenge 64".to_string()),
            format: Some("CMODERN".to_string()),
            status,
            finish: None,
            round: Some(4),
            stage: StandingStage::Swiss,
            swiss_rounds: Some(6),
            cut_size: Some(8),
            rank: Some(12),
            players: Some(64),
            matches_left: Some(12),
            byes: 0,
            place: None,
            starts_at_ms: Some(NOW - 3 * HOUR),
            round_result_at_ms: None,
            finished_at_ms: None,
            last_transition_at_ms: NOW - 10 * MINUTE,
            last_event_at_ms: NOW - MINUTE,
            matches: Vec::new(),
            registered_deck: None,
            format_deck: None,
        }
    }

    fn played(id: i64, round: u32, state: MatchState, outcome: Outcome) -> TournamentMatchInput {
        TournamentMatchInput {
            round: Some(round),
            deck: None,
            member: member(id, state, outcome, NOW - (10 - i64::from(round)) * HOUR),
        }
    }

    /// Three won rounds and round 4 live in game 1.
    fn in_round_four() -> TournamentInput {
        let mut event = tournament(StandingStatus::Playing);
        for round in 1..4 {
            event.matches.push(played(
                round,
                round as u32,
                MatchState::Complete,
                Outcome::Win,
            ));
        }
        let mut live = played(4, 4, MatchState::InProgress, Outcome::Unknown);
        live.member.games = vec![None];
        live.member.newest_game_start = Some((1, 900));
        event.matches.push(live);
        event
    }

    fn tournament_card_of(
        runs: Vec<LeagueRunInput>,
        tournaments: Vec<TournamentInput>,
        strip: bool,
        now_ms: i64,
    ) -> LeagueOverlayState {
        let look = CardLook { strip, ..look() };
        league_state(
            LeagueCardInput {
                runs,
                tournaments,
                ..LeagueCardInput::default()
            },
            now_ms,
            GRACE_MS,
            &look,
        )
    }

    fn one_tournament(event: TournamentInput) -> LeagueOverlayState {
        tournament_card_of(Vec::new(), vec![event], true, NOW)
    }

    #[test]
    fn the_new_enums_have_the_spec_spellings() {
        let json = |value: serde_json::Value| value.as_str().expect("text").to_string();
        assert_eq!(
            json(serde_json::to_value(TournamentStage::Top8).unwrap()),
            "top8"
        );
        assert_eq!(
            json(serde_json::to_value(TournamentSeries::ShowcaseChallenge).unwrap()),
            "showcase_challenge"
        );
        assert_eq!(
            json(serde_json::to_value(LeagueStatus::WaitingForRound).unwrap()),
            "waiting_for_round"
        );
        assert_eq!(
            json(serde_json::to_value(LeagueStatus::MadeCut).unwrap()),
            "made_cut"
        );
        assert_eq!(
            json(serde_json::to_value(EventKind::Tournament).unwrap()),
            "tournament"
        );
    }

    #[test]
    fn a_tournament_in_game_shows_its_round_record_and_numbers() {
        let state = one_tournament(in_round_four());
        assert_eq!(state.status, LeagueStatus::InGame);
        let event = state.event.clone().expect("event");
        assert_eq!(event.kind, EventKind::Tournament);
        assert_eq!(event.name, "Modern Challenge 64");
        assert_eq!(event.format, "Modern");
        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 3,
                losses: 0,
                draws: None
            })
        );
        assert_eq!(
            state.progress, None,
            "the round lives in the tournament block"
        );
        assert_eq!(state.live_match.as_ref().map(|live| live.number), Some(4));
        let block = state.tournament.expect("block");
        assert_eq!(block.series, TournamentSeries::Challenge);
        assert_eq!((block.round, block.stage), (4, TournamentStage::Swiss));
        assert_eq!((block.swiss_rounds, block.cut_size), (Some(6), Some(8)));
        assert_eq!((block.rank, block.players), (Some(12), Some(64)));
        assert_eq!(block.matches_left, None, "only while waiting");
        assert_eq!(block.next_round_at_ms, None);
        assert_eq!(block.starts_at_ms, None);
        let json = serde_json::to_value(one_tournament(in_round_four())).expect("json");
        assert!(json["record"].get("draws").is_none(), "no draws: no key");
        assert!(json.get("other").is_none());
    }

    #[test]
    fn waiting_while_the_round_runs_has_matches_left_and_no_estimate() {
        let mut event = in_round_four();
        event.status = StandingStatus::WaitingForRound;
        event.matches.last_mut().expect("round 4").member.state = MatchState::Complete;
        event.matches.last_mut().expect("round 4").member.outcome = Outcome::Win;
        let block = one_tournament(event.clone()).tournament.expect("block");
        assert_eq!(block.matches_left, Some(12));
        assert_eq!(
            block.next_round_at_ms, None,
            "no estimate while a round runs"
        );

        event.matches_left = Some(0);
        event.round_result_at_ms = Some(NOW - MINUTE);
        let state = one_tournament(event);
        assert_eq!(state.status, LeagueStatus::WaitingForRound);
        let block = state.tournament.expect("block");
        assert_eq!(block.next_round_at_ms, Some(NOW - MINUTE + PAIRING_GAP_MS));
        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 4,
                losses: 0,
                draws: None
            })
        );
    }

    #[test]
    fn byes_count_as_wins_and_draws_are_sent_only_when_there_are_some() {
        let mut event = tournament(StandingStatus::WaitingForRound);
        event.byes = 1;
        event
            .matches
            .push(played(1, 2, MatchState::Complete, Outcome::Draw));
        event
            .matches
            .push(played(2, 3, MatchState::Complete, Outcome::Loss));
        let state = one_tournament(event);
        assert_eq!(
            state.record,
            Some(LeagueRecord {
                wins: 1,
                losses: 1,
                draws: Some(1)
            })
        );
    }

    #[test]
    fn a_registered_tournament_has_its_start_and_its_registered_deck() {
        let mut event = tournament(StandingStatus::Registered);
        event.round = None;
        event.starts_at_ms = Some(NOW + 12 * MINUTE);
        let mut format_deck = deck();
        format_deck.id = 8;
        event.format_deck = Some(format_deck);
        let state = one_tournament(event.clone());
        assert_eq!(state.status, LeagueStatus::Registered);
        assert_eq!(
            state.tournament.expect("block").starts_at_ms,
            Some(NOW + 12 * MINUTE)
        );
        assert_eq!(
            state.deck.as_ref().map(|deck| deck.id),
            Some(8),
            "no registered deck: the last played deck of the format"
        );
        event.registered_deck = Some(deck());
        assert_eq!(one_tournament(event).deck.map(|deck| deck.id), Some(7));
    }

    #[test]
    fn the_current_rounds_match_deck_comes_first() {
        let mut event = in_round_four();
        event.registered_deck = Some(deck());
        let mut round_deck = deck();
        round_deck.id = 9;
        event.matches.last_mut().expect("round 4").deck = Some(round_deck);
        assert_eq!(one_tournament(event).deck.map(|deck| deck.id), Some(9));
    }

    #[test]
    fn a_top_cut_finish_shows_its_place_and_game_record() {
        let mut event = in_round_four();
        event.matches.last_mut().expect("round 4").member.state = MatchState::Complete;
        event.matches.last_mut().expect("round 4").member.outcome = Outcome::Loss;
        event.status = StandingStatus::Finished;
        event.finish = Some(StandingFinish::TopCut);
        event.stage = StandingStage::Top8;
        event.place = Some(6);
        event.finished_at_ms = Some(NOW - MINUTE);
        let state = one_tournament(event);
        assert_eq!(state.status, LeagueStatus::TopCut);
        assert_eq!(state.live_match, None);
        assert!(state.game_record.is_some());
        let block = state.tournament.expect("block");
        assert_eq!((block.place, block.stage), (Some(6), TournamentStage::Top8));
    }

    #[test]
    fn every_finish_has_its_status() {
        for (finish, status) in [
            (StandingFinish::Trophied, LeagueStatus::Trophied),
            (StandingFinish::TopCut, LeagueStatus::TopCut),
            (StandingFinish::MissedCut, LeagueStatus::MissedCut),
            (StandingFinish::Complete, LeagueStatus::Complete),
            (StandingFinish::Dropped, LeagueStatus::Dropped),
            (StandingFinish::Eliminated, LeagueStatus::Eliminated),
        ] {
            let mut event = tournament(StandingStatus::Finished);
            event.finish = Some(finish);
            event.place = Some(9);
            event.finished_at_ms = Some(NOW - MINUTE);
            let state = one_tournament(event);
            assert_eq!(state.status, status);
            let placed = matches!(
                status,
                LeagueStatus::Trophied
                    | LeagueStatus::TopCut
                    | LeagueStatus::MissedCut
                    | LeagueStatus::Complete
            );
            assert_eq!(state.tournament.expect("block").place.is_some(), placed);
        }
    }

    #[test]
    fn a_stale_tournament_leaves_the_card_and_the_wake_time_says_when() {
        let event = tournament(StandingStatus::WaitingForRound);
        let leaves = event.last_event_at_ms + STALE_AFTER_MS;
        let input = LeagueCardInput {
            tournaments: vec![event.clone()],
            ..LeagueCardInput::default()
        };
        assert_eq!(changes_at(&input, NOW, GRACE_MS), Some(leaves));
        let shown = tournament_card_of(Vec::new(), vec![event.clone()], true, leaves - 1);
        assert_eq!(shown.status, LeagueStatus::WaitingForRound);
        let gone = tournament_card_of(Vec::new(), vec![event], true, leaves);
        assert_eq!(gone.status, LeagueStatus::Idle);
    }

    #[test]
    fn a_finished_tournament_leaves_after_the_finished_time() {
        let mut event = tournament(StandingStatus::Finished);
        event.finish = Some(StandingFinish::MissedCut);
        event.finished_at_ms = Some(NOW - MINUTE);
        let leaves = NOW - MINUTE + GRACE_MS + 1;
        let input = LeagueCardInput {
            tournaments: vec![event.clone()],
            ..LeagueCardInput::default()
        };
        assert_eq!(changes_at(&input, NOW, GRACE_MS), Some(leaves));
        let gone = tournament_card_of(Vec::new(), vec![event], true, leaves);
        assert_eq!(gone.status, LeagueStatus::Idle);
    }

    #[test]
    fn a_registered_tournament_never_paired_leaves_thirty_minutes_after_its_start() {
        let mut event = tournament(StandingStatus::Registered);
        event.starts_at_ms = Some(NOW - 29 * MINUTE);
        // Stale by its last fact already: the start rule is the later one.
        event.last_event_at_ms = NOW - 100 * MINUTE;
        let input = LeagueCardInput {
            tournaments: vec![event.clone()],
            ..LeagueCardInput::default()
        };
        let leaves = NOW + MINUTE;
        assert_eq!(changes_at(&input, NOW, GRACE_MS), Some(leaves));
        let gone = tournament_card_of(Vec::new(), vec![event], true, leaves);
        assert_eq!(gone.status, LeagueStatus::Idle);
    }

    #[test]
    fn a_live_league_takes_the_card_and_the_tournament_goes_to_the_strip() {
        let mut run = run_with_results(&[Outcome::Win]);
        let mut live = live_member(9);
        live.games = vec![Some(true), Some(false)];
        live.sideboarding_events = vec![((1, 950), "TournamentMatchSideboardingState".to_string())];
        live.newest_game_start = Some((1, 900));
        run.matches.push(live);
        let waiting = tournament(StandingStatus::WaitingForRound);

        let state = tournament_card_of(vec![run.clone()], vec![waiting.clone()], true, NOW);
        assert_eq!(
            state.event.as_ref().map(|event| event.kind),
            Some(EventKind::League)
        );
        let other = state.other.expect("the strip");
        assert_eq!(other.kind, EventKind::Tournament);
        assert_eq!(other.name, "Modern Challenge 64");
        assert_eq!(other.status, LeagueStatus::WaitingForRound);
        assert_eq!(other.live_match, None);
        assert!(other.tournament.is_some());

        let off = tournament_card_of(vec![run], vec![waiting], false, NOW);
        assert_eq!(off.other, None, "the strip switch is off");
    }

    #[test]
    fn a_live_tournament_beats_a_live_league_and_the_league_match_goes_to_the_strip() {
        let mut run = run_with_results(&[Outcome::Win]);
        let mut live = live_member(9);
        live.games = vec![Some(true), None];
        live.newest_game_start = Some((1, 900));
        run.matches.push(live);
        let state = tournament_card_of(vec![run], vec![in_round_four()], true, NOW);
        assert_eq!(
            state.event.as_ref().map(|event| event.kind),
            Some(EventKind::Tournament)
        );
        let other = state.other.expect("the strip");
        assert_eq!(other.kind, EventKind::League);
        assert_eq!(
            other.live_match,
            Some(OtherMatch {
                game: 2,
                games_won: 1,
                games_lost: 0
            })
        );
    }

    #[test]
    fn matches_left_never_flips_the_card() {
        let mut first = tournament(StandingStatus::WaitingForRound);
        first.last_transition_at_ms = NOW - 5 * MINUTE;
        let mut second = tournament(StandingStatus::WaitingForRound);
        second.mtgo_event_id = EVENT_ID + 1;
        second.name = Some("Legacy Challenge 32".to_string());
        second.last_transition_at_ms = NOW - 6 * MINUTE;
        let before = tournament_card_of(Vec::new(), vec![first.clone(), second.clone()], true, NOW);
        second.matches_left = Some(1);
        second.last_event_at_ms = NOW;
        let after = tournament_card_of(Vec::new(), vec![first, second], true, NOW);
        assert_eq!(before.event, after.event);
        assert_eq!(
            before.event.map(|event| event.name),
            Some("Modern Challenge 64".to_string())
        );
    }

    #[test]
    fn the_shown_live_match_of_a_tournament_is_its_rounds_match() {
        let input = LeagueCardInput {
            tournaments: vec![in_round_four()],
            ..LeagueCardInput::default()
        };
        assert_eq!(shown_live_match_id(&input, NOW, GRACE_MS), Some(4));
        assert_eq!(
            shown_live_match_token(&input, NOW, GRACE_MS),
            Some("token-4".to_string())
        );
        let tokens: Vec<String> = live_tokens(&input).into_iter().collect();
        assert_eq!(tokens, vec!["token-4".to_string()]);
    }

    #[test]
    fn the_series_comes_from_the_event_name() {
        for (name, series) in [
            ("Modern Challenge 64", TournamentSeries::Challenge),
            (
                "Legacy Showcase Challenge",
                TournamentSeries::ShowcaseChallenge,
            ),
            ("Modern Showcase Qualifier", TournamentSeries::Qualifier),
            ("Pioneer Preliminary", TournamentSeries::Preliminary),
            ("Vintage Cube Draft", TournamentSeries::Other),
        ] {
            assert_eq!(tournament_series(name), series, "{name}");
        }
    }

    #[test]
    fn a_match_stuck_in_an_earlier_round_does_not_make_the_tournament_live() {
        let mut event = tournament(StandingStatus::WaitingForRound);
        event.round = Some(5);
        event
            .matches
            .push(played(3, 3, MatchState::InProgress, Outcome::Unknown));
        let state = one_tournament(event.clone());
        assert_eq!(state.status, LeagueStatus::WaitingForRound);
        assert_eq!(state.live_match, None);
        let input = LeagueCardInput {
            tournaments: vec![event],
            ..LeagueCardInput::default()
        };
        assert_eq!(shown_live_match_id(&input, NOW, GRACE_MS), None);
        assert!(live_tokens(&input).is_empty());
    }

    #[test]
    fn an_early_registration_stays_until_thirty_minutes_after_the_start() {
        let mut event = tournament(StandingStatus::Registered);
        event.round = None;
        event.last_event_at_ms = NOW - 100 * MINUTE;
        event.starts_at_ms = Some(NOW + HOUR);
        let input = LeagueCardInput {
            tournaments: vec![event.clone()],
            ..LeagueCardInput::default()
        };
        let leaves = NOW + HOUR + UNPAIRED_AFTER_START_MS;
        assert_eq!(changes_at(&input, NOW, GRACE_MS), Some(leaves));
        assert_eq!(
            one_tournament(event.clone()).status,
            LeagueStatus::Registered
        );
        let gone = tournament_card_of(Vec::new(), vec![event], true, leaves);
        assert_eq!(gone.status, LeagueStatus::Idle);
    }
}
