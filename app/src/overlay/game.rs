//! The game overlay's view (overlay spec 7; 0.x `GameOverlayController`):
//! what the `game-overlay` window shows for the live match.
//!
//! - **Frame parts** (draw odds, reveals, sideboarding) follow every
//!   committed frame of the live match.
//! - **Match parts** (the opponent and their archetype, the head-to-head
//!   record, the cards the archetype could still have, the sideboard guide
//!   and the matchup notes) are computed on the live match's other changes:
//!   a match, archetype, guide, deck or settings event, or a net answer, and
//!   when the reveals change. A frame that leaves the reveals as they were
//!   keeps them (0.x deferred the draw odds so a poll never computed them;
//!   v1 pushes, so it is the other way round: a frame recomputes only what
//!   a frame changes).
//!
//! Sections switched off are not computed (0.x "omits disabled sections");
//! the opponent is resolved whenever the header, the sideboard guide or the
//! reveals are on, since all three key on the archetype, and sent only with
//! the header on. Nothing here waits on the network: `GameLookups` answers
//! from what is kept and asks for what is missing.

use std::collections::BTreeMap;

use archetypes::guides::VersionCard;
use archetypes::{ObservedCard, format_key, rules_match, score};
use decks::DeckCard;
use game_detail::frame::Frame;
use overlay::height::Sections;
use overlay::{
    ArchetypeRef, CardInfo, CardMeta, DrawOdds, GuideStats, ListEntry, LiveEstimate,
    OpponentSources, PotentialCards, RevealedCard, SideboardGuide, Source,
};
use serde::Serialize;
use store::overlay::{LiveGame, LiveGameInputs};
use store::{Connection, StoreError};

use crate::overlay::game_net::{GameLookups, GameNet, LocalOnly};
use crate::settings::Settings;

/// The game overlay's view of the live match.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GameOverlayView {
    /// Which sections are switched on.
    pub sections: Sections,
    /// A live match is stored ("Waiting for match…" without one).
    pub has_match: bool,
    /// The live match is linked to a deck version (0.x `hasDeck`).
    pub has_deck: bool,
    /// The opponent's archetype resolved to one v1 holds (0.x
    /// `hasArchetype`), whatever the section toggles.
    pub has_archetype: bool,
    /// The match's format, for display (`Modern`).
    pub format: Option<String>,
    /// The window is collapsed to its tab bar.
    pub collapsed: bool,
    /// With the header on and an opponent known.
    pub opponent: Option<OverlayOpponent>,
    /// With the header on and a game started: the current game's play or
    /// draw and hand sizes.
    pub game: Option<OverlayGame>,
    /// With the draw odds section on and a deck list.
    pub draw_odds: Option<DrawOdds>,
    /// With the reveals section on: the opponent's cards across the match.
    pub reveals: Option<Vec<RevealedCard>>,
    /// With the reveals section on and an archetype known: what its lists
    /// play that has not been revealed.
    pub potential: Option<PotentialCards>,
    /// With the sideboard section on, a deck and a held archetype.
    pub sideboard: Option<SideboardGuide>,
    /// With the sideboard section on: the matchup's notes.
    pub notes: OverlayNotes,
    /// With the sideboard section on: the current game has ended and the
    /// next has not started.
    pub is_sideboarding: bool,
}

/// The opponent header (0.x `OverlayOpponentData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlayOpponent {
    pub username: String,
    /// Complete matches against them in this format, the live one left out.
    pub previous_matches: u32,
    pub wins: u32,
    pub losses: u32,
    /// `Met {n}× · won {w}`, `None` without earlier matches.
    pub head_to_head: Option<String>,
    /// The archetype v1 holds; `None` for one only the API named.
    #[specta(type = Option<specta_typescript::Number>)]
    pub archetype_id: Option<i64>,
    pub archetype_uuid: Option<String>,
    /// `None` shows "Unknown archetype", or "Couldn't reach MyMTGO" when
    /// `unreachable`.
    pub archetype_name: Option<String>,
    /// Colour identity letters in WUBRG order.
    pub archetype_colors: Vec<String>,
    pub source: Source,
    /// "5-0 list", "last played", "seen by others", "guess", "you faced";
    /// none for a pick, a live read or no archetype.
    pub source_label: Option<String>,
    pub manual: bool,
    /// No archetype, and the latest lookup could not reach MyMTGO (#503):
    /// it is asked again, so the answer may still come.
    pub unreachable: bool,
}

/// The current game above the opponent's name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlayGame {
    /// The local player is on the play; `None` before the choice is logged.
    pub on_play: Option<bool>,
    /// The hand each player kept (7 less their mulligans); `None` until
    /// their keep is logged.
    pub local_hand: Option<u32>,
    pub opponent_hand: Option<u32>,
}

/// The matchup notes (0.x `GetArchetypeNotes`), newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlayNotes {
    /// This deck's ("Notes for this deck", deletable).
    pub current: Vec<OverlayNote>,
    /// Other decks' ("Other notes", "{deck} · {date}").
    pub other: Vec<OverlayNote>,
}

/// One matchup note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlayNote {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    pub deck_name: String,
    pub body: String,
    /// UTC ms.
    #[specta(type = specta_typescript::Number)]
    pub created_at_ms: i64,
}

/// What the commands need of the live match (found in Rust, never taken
/// from a request: 0.x).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveContext {
    pub match_id: i64,
    /// MTGO's format code (`CMODERN`).
    pub format: Option<String>,
    pub opponent_id: Option<i64>,
    pub deck_id: Option<i64>,
    /// The opponent's archetype, when v1 holds it.
    pub archetype_id: Option<i64>,
}

/// The match parts of the last full compute, kept for frame-only wakes.
#[derive(Debug, Clone, PartialEq)]
pub struct GameMemo {
    token: String,
    /// The reveals the match parts were computed with.
    fingerprint: String,
    has_archetype: bool,
    /// The archetype's name as resolved, whatever the section toggles.
    archetype_name: Option<String>,
    opponent: Option<OverlayOpponent>,
    potential: Option<PotentialCards>,
    sideboard: Option<SideboardGuide>,
    notes: OverlayNotes,
    pub live: LiveContext,
}

/// The archetype the game overlay named for a live match, handed to the
/// league card so both say the same thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownOpponent {
    pub token: String,
    pub archetype: String,
}

impl GameMemo {
    /// The resolved archetype of this memo's match, when it has a name.
    pub fn known_opponent(&self) -> Option<KnownOpponent> {
        let archetype = self.archetype_name.clone()?;
        Some(KnownOpponent {
            token: self.token.clone(),
            archetype,
        })
    }
}

/// The sections switched on in the settings.
pub fn sections(settings: &Settings) -> Sections {
    Sections {
        opponent: settings.overlay_show_opponent,
        draw_odds: settings.overlay_show_draw_odds,
        reveals: settings.overlay_show_reveals,
        sideboard: settings.overlay_show_sideboard,
    }
}

/// The view with no live match.
pub fn empty_view(settings: &Settings, collapsed: bool) -> GameOverlayView {
    GameOverlayView {
        sections: sections(settings),
        has_match: false,
        has_deck: false,
        has_archetype: false,
        format: None,
        collapsed,
        opponent: None,
        game: None,
        draw_odds: None,
        reveals: None,
        potential: None,
        sideboard: None,
        notes: OverlayNotes::default(),
        is_sideboarding: false,
    }
}

/// What a game view is computed from besides the database.
pub struct GameInputs<'a> {
    pub settings: &'a Settings,
    pub collapsed: bool,
    pub now_ms: i64,
    /// The last full compute, on a wake that only stored frames: its match
    /// parts are kept when the token and the reveals are the same.
    pub previous: Option<&'a GameMemo>,
}

/// The view of the live match `token` (`None`: no live match) and the
/// memo of its match parts (`None` without a live match).
pub fn game_view(
    conn: &Connection,
    token: Option<&str>,
    inputs: &GameInputs<'_>,
    lookups: &mut dyn GameLookups,
) -> Result<(GameOverlayView, Option<GameMemo>), StoreError> {
    let mut view = empty_view(inputs.settings, inputs.collapsed);
    let Some(token) = token else {
        return Ok((view, None));
    };
    let Some(live) = store::overlay::live_game_inputs(conn, token)? else {
        return Ok((view, None));
    };
    view.has_match = true;
    view.has_deck = live.deck_id.is_some();
    view.format = crate::summary::format_label(live.format.as_deref());

    let current = live.games.last();
    let deck = current_deck(conn, &live, current)?;
    let frames = newest_frames(&live);
    let revealed = revealed_cards(&frames, &live.local_username);
    let cards = card_meta(conn, &deck, &frames)?;

    let sections = view.sections;
    let current_frame = frames.last().and_then(Option::as_ref);
    if sections.draw_odds {
        // Before the play or draw is chosen the frame already holds the
        // dealt hand: the odds count the whole deck so they do not give it
        // away.
        let choosing = current.is_none_or(|game| {
            overlay::before_play_draw(game.on_play, game.local_mulligans, current_frame)
        });
        let frame = if choosing { None } else { current_frame };
        view.draw_odds = draw_odds(&deck, frame, &live.local_username, &cards);
    }
    if sections.opponent
        && let Some(game) = current
    {
        view.game = Some(current_game(game));
    }
    let reveals = overlay::merge_by_name(&revealed, &cards);
    if sections.reveals {
        view.reveals = Some(reveals.clone());
    }
    if sections.sideboard {
        let newest_game_start = current.and_then(|game| game.first_frame_at);
        let events = store::overlay::sideboarding_events(conn, token)?;
        view.is_sideboarding = overlay::is_sideboarding(&events, newest_game_start);
    }

    let fingerprint = overlay::fingerprint(&revealed);
    let memo = match inputs.previous {
        Some(previous) if previous.token == token && previous.fingerprint == fingerprint => {
            previous.clone()
        }
        _ => match_parts(
            conn,
            token,
            &live,
            &revealed,
            &reveals,
            &cards,
            fingerprint,
            inputs,
            lookups,
        )?,
    };
    view.has_archetype = memo.has_archetype;
    view.opponent = memo.opponent.clone();
    view.potential = memo.potential.clone();
    view.sideboard = memo.sideboard.clone();
    view.notes = memo.notes.clone();
    Ok((view, Some(memo)))
}

/// The league card's name for the opponent of its live match `token`: the
/// game overlay's answer when it resolved this match (`known`), else the
/// sources held locally, through `net`'s kept candidates and live reads:
/// a manual pick, the opponent last faced and the live read of their
/// reveals. Nothing is ever asked of the network for the card, and the
/// name is `None` when nothing answers.
pub fn card_opponent_archetype(
    conn: &Connection,
    token: &str,
    known: Option<&KnownOpponent>,
    now_ms: i64,
    net: &mut GameNet,
) -> Result<Option<String>, StoreError> {
    if let Some(known) = known
        && known.token == token
    {
        return Ok(Some(known.archetype.clone()));
    }
    let Some(live) = store::overlay::live_game_inputs(conn, token)? else {
        return Ok(None);
    };
    let (Some(opponent_id), Some(username)) = (live.opponent_id, live.opponent_username.as_ref())
    else {
        return Ok(None);
    };
    let frames = newest_frames(&live);
    let revealed = revealed_cards(&frames, &live.local_username);
    let cards = card_meta(conn, &[], &frames)?;
    let resolved = resolve(
        conn,
        &live,
        opponent_id,
        username,
        &revealed,
        &cards,
        now_ms,
        &mut LocalOnly(net),
    )?;
    Ok(shown_archetype(conn, resolved.archetype.as_ref())?.name)
}

/// The parts that follow the match rather than its frames.
#[allow(clippy::too_many_arguments)]
fn match_parts(
    conn: &Connection,
    token: &str,
    live: &LiveGameInputs,
    revealed: &[(u32, u32)],
    reveals: &[RevealedCard],
    cards: &CardMeta,
    fingerprint: String,
    inputs: &GameInputs<'_>,
    lookups: &mut dyn GameLookups,
) -> Result<GameMemo, StoreError> {
    let sections = sections(inputs.settings);
    let mut memo = GameMemo {
        token: token.to_string(),
        fingerprint,
        has_archetype: false,
        archetype_name: None,
        opponent: None,
        potential: None,
        sideboard: None,
        notes: OverlayNotes::default(),
        live: LiveContext {
            match_id: live.match_id,
            format: live.format.clone(),
            opponent_id: live.opponent_id,
            deck_id: live.deck_id,
            archetype_id: None,
        },
    };
    if !(sections.opponent || sections.sideboard || sections.reveals) {
        return Ok(memo);
    }
    let (Some(opponent_id), Some(username)) = (live.opponent_id, live.opponent_username.as_ref())
    else {
        // "Waiting for opponent…".
        return Ok(memo);
    };
    let resolved = resolve(
        conn,
        live,
        opponent_id,
        username,
        revealed,
        cards,
        inputs.now_ms,
        lookups,
    )?;
    let shown = shown_archetype(conn, resolved.archetype.as_ref())?;
    memo.has_archetype = shown.held_id.is_some();
    memo.archetype_name = shown.name.clone();
    memo.live.archetype_id = shown.held_id;

    if sections.opponent {
        let (wins, losses) =
            store::overlay::head_to_head(conn, opponent_id, live.format.as_deref(), live.match_id)?;
        let label = if shown.name.is_some() {
            resolved.source.label().map(str::to_string)
        } else {
            None
        };
        memo.opponent = Some(OverlayOpponent {
            username: username.clone(),
            previous_matches: wins + losses,
            wins,
            losses,
            head_to_head: overlay::head_to_head_text(wins + losses, wins),
            archetype_id: shown.held_id,
            archetype_uuid: shown.uuid.clone(),
            archetype_name: shown.name.clone(),
            archetype_colors: crate::summary::archetype_colors(shown.colors.as_deref()),
            source: resolved.source,
            source_label: label,
            manual: resolved.manual,
            unreachable: shown.name.is_none() && lookups.unreachable(),
        });
    }

    if sections.reveals && resolved.archetype.is_some() {
        memo.potential = potential(conn, &shown, reveals, inputs.now_ms, lookups)?;
    }

    if sections.sideboard
        && let (Some(deck_id), Some(version_id), Some(archetype_id)) =
            (live.deck_id, live.deck_version_id, shown.held_id)
    {
        memo.sideboard = Some(sideboard_guide(conn, deck_id, version_id, archetype_id)?);
        memo.notes = notes(conn, deck_id, archetype_id)?;
    }
    Ok(memo)
}

/// Every source of the opponent's archetype, in 0.x's order (spec 7.3).
#[allow(clippy::too_many_arguments)]
fn resolve(
    conn: &Connection,
    live: &LiveGameInputs,
    opponent_id: i64,
    username: &str,
    revealed: &[(u32, u32)],
    cards: &CardMeta,
    now_ms: i64,
    lookups: &mut dyn GameLookups,
) -> Result<overlay::Resolved, StoreError> {
    let key = live.format.as_deref().and_then(format_key);
    let mut sources = OpponentSources {
        manual: store::overlay::manual_opponent_archetype(conn, live.match_id)?
            .map(ArchetypeRef::Held),
        ..OpponentSources::default()
    };
    if sources.manual.is_some() {
        return Ok(overlay::resolve_opponent(sources));
    }

    let set = format!("{}:{}", live.match_id, overlay::fingerprint(revealed));
    if !revealed.is_empty()
        && let Some(key) = key.as_deref()
    {
        // Kept between computes while the format's archetypes stay the same
        // (spec 14: reading them is most of a full compute).
        let stamp = store::overlay::candidates_stamp(conn, key)?;
        let candidates = lookups.held_candidates(key, stamp, &mut || {
            store::archetypes::held_candidates(conn, key)
        })?;
        let mut failure: Option<StoreError> = None;
        let mut compute = || match live_read(conn, &candidates, revealed, cards) {
            Ok(estimate) => estimate,
            Err(error) => {
                failure = Some(error);
                None
            }
        };
        sources.live = lookups.live_read(&set, now_ms, &mut compute);
        if let Some(error) = failure {
            return Err(error);
        }
    }

    sources.last_faced = store::overlay::last_faced_archetype(
        conn,
        opponent_id,
        live.format.as_deref(),
        live.match_id,
    )?
    .map(ArchetypeRef::Held);

    if let Some(key) = key.as_deref() {
        if let Some(scouting) = lookups.scouting(username, key, now_ms) {
            sources.league = scouted(conn, scouting.league.as_ref(), key, now_ms, lookups)?;
            sources.tracked = scouted(conn, scouting.tracked.as_ref(), key, now_ms, lookups)?;
            sources.observed = scouted(conn, scouting.observed.as_ref(), key, now_ms, lookups)?;
        }
        // Only when nothing above the estimate answers, so a held answer
        // never costs a request (0.x asked in order too).
        let above = sources.last_faced.is_some()
            || sources.league.is_some()
            || sources.tracked.is_some()
            || sources
                .live
                .as_ref()
                .is_some_and(|live| live.confidence >= overlay::LIVE_CONFIDENCE_THRESHOLD);
        if !above
            && !revealed.is_empty()
            && let Some(format) = live.format.as_deref()
            && let Some(estimate) = lookups.estimate(live.match_id, &set, format, revealed, now_ms)
        {
            sources.api = named(conn, &estimate.uuid, None, key, now_ms, lookups)?;
        }
    }
    Ok(overlay::resolve_opponent(sources))
}

/// The live local read of the reveals (spec 7.3): the rules, else the
/// local estimate, over the format's held `candidates`.
fn live_read(
    conn: &Connection,
    candidates: &[archetypes::Candidate],
    revealed: &[(u32, u32)],
    cards: &CardMeta,
) -> Result<Option<LiveEstimate>, StoreError> {
    if candidates.is_empty() {
        return Ok(None);
    }
    let observed: Vec<ObservedCard> = revealed
        .iter()
        .map(|(catalog_id, quantity)| {
            let info = cards.get(*catalog_id);
            ObservedCard {
                catalog_id: i64::from(*catalog_id),
                quantity: *quantity,
                sideboard: false,
                oracle_id: info.and_then(|info| info.oracle_id.clone()),
                name: info.and_then(|info| info.name.clone()),
                card_type: info.and_then(|info| info.card_type.clone()),
                color_identity: info.and_then(|info| info.color_identity.clone()),
                mana_cost: info.and_then(|info| info.mana_cost.clone()),
            }
        })
        .collect();
    let answer = match rules_match(&observed, candidates) {
        Some(answer) => Some(answer),
        None => score(&observed, candidates)
            .map(|estimate| (estimate.archetype_id, estimate.confidence)),
    };
    let Some((archetype_id, confidence)) = answer else {
        return Ok(None);
    };
    Ok(Some(LiveEstimate {
        archetype: ArchetypeRef::Held(hop(conn, archetype_id)?),
        confidence,
    }))
}

/// A scouting source's archetype: the one v1 holds by uuid (0.x resolved
/// by uuid only), else the API's name, handed to Plan T as a stub.
fn scouted(
    conn: &Connection,
    archetype: Option<&mymtgo_api::ScoutedArchetype>,
    format_key: &str,
    now_ms: i64,
    lookups: &mut dyn GameLookups,
) -> Result<Option<ArchetypeRef>, StoreError> {
    let Some(archetype) = archetype else {
        return Ok(None);
    };
    named(
        conn,
        &archetype.uuid,
        Some(&archetype.name),
        format_key,
        now_ms,
        lookups,
    )
}

/// An archetype the API named by uuid: held (one merge hop), else shown by
/// the API's name (or the metagame list's) and handed to Plan T. A retired
/// one answers nothing (as detection).
fn named(
    conn: &Connection,
    uuid: &str,
    name: Option<&str>,
    format_key: &str,
    now_ms: i64,
    lookups: &mut dyn GameLookups,
) -> Result<Option<ArchetypeRef>, StoreError> {
    if let Some(held) = store::archetypes::held_uuid(conn, uuid)? {
        if held.retired {
            return Ok(None);
        }
        return Ok(Some(ArchetypeRef::Held(
            held.merged_into_id.unwrap_or(held.id),
        )));
    }
    let listed = lookups.metagame_name(format_key, uuid, now_ms);
    let name = name
        .map(str::to_string)
        .or_else(|| listed.as_ref().map(|(name, _)| name.clone()));
    let colors = listed.and_then(|(_, colors)| colors);
    lookups.ensure_stub(
        uuid,
        name.as_deref().unwrap_or(""),
        format_key,
        colors.as_deref(),
    );
    Ok(name.map(|name| ArchetypeRef::Unheld {
        uuid: uuid.to_string(),
        name,
    }))
}

/// One merge hop (0.x `ResolveMergedArchetype`).
fn hop(conn: &Connection, archetype_id: i64) -> Result<i64, StoreError> {
    let parent = store::archetypes::merged_into(conn, archetype_id)?;
    Ok(archetypes::merge_hop(archetype_id, |_| parent))
}

/// The archetype as the header shows it.
#[derive(Debug, Clone, Default, PartialEq)]
struct Shown {
    held_id: Option<i64>,
    uuid: Option<String>,
    name: Option<String>,
    colors: Option<String>,
    downloaded: bool,
}

fn shown_archetype(
    conn: &Connection,
    archetype: Option<&ArchetypeRef>,
) -> Result<Shown, StoreError> {
    match archetype {
        None => Ok(Shown::default()),
        Some(ArchetypeRef::Unheld { uuid, name }) => Ok(Shown {
            uuid: Some(uuid.clone()),
            name: Some(name.clone()),
            ..Shown::default()
        }),
        Some(ArchetypeRef::Held(id)) => {
            let Some(row) = store::archetypes::archetype(conn, *id)? else {
                return Ok(Shown::default());
            };
            // A stub named only by its uuid waits for its definition's name.
            let name = (row.name != row.uuid).then(|| row.name.clone());
            Ok(Shown {
                held_id: Some(row.id),
                uuid: Some(row.uuid),
                name,
                colors: row.color_identity,
                downloaded: row.source == archetypes::Source::Downloaded,
            })
        }
    }
}

/// The cards the archetype could still have (spec 7.5): over its live
/// decklists when fresh, else the held key list (list 1) of a downloaded
/// archetype or every variant of a manual one.
fn potential(
    conn: &Connection,
    shown: &Shown,
    reveals: &[RevealedCard],
    now_ms: i64,
    lookups: &mut dyn GameLookups,
) -> Result<Option<PotentialCards>, StoreError> {
    let revealed: BTreeMap<String, u32> = reveals
        .iter()
        .map(|card| (card.name.clone(), card.quantity))
        .collect();
    let live_lists = match shown.uuid.as_deref() {
        Some(uuid) if lookups.online() && !(shown.held_id.is_some() && !shown.downloaded) => {
            lookups.decklists(uuid, now_ms)
        }
        _ => None,
    };
    let mut lists: Vec<Vec<ListEntry>> = Vec::new();
    let mut ids: Vec<u32> = Vec::new();
    match (live_lists, shown.held_id) {
        (Some(decks), _) => {
            for deck in decks {
                let mut list = Vec::new();
                for card in deck.cards {
                    ids.push(card.mtgo_id);
                    list.push(ListEntry {
                        catalog_id: Some(card.mtgo_id),
                        name: card.name.unwrap_or_else(|| format!("#{}", card.mtgo_id)),
                        card_type: card.card_type,
                        quantity: card.quantity,
                        sideboard: card.sideboard,
                    });
                }
                lists.push(list);
            }
        }
        (None, Some(archetype_id)) => {
            let mut held = store::archetypes::archetype_lists(conn, archetype_id)?;
            if shown.downloaded {
                held.truncate(1);
            }
            let wanted: Vec<u32> = held
                .iter()
                .flat_map(|list| list.cards.iter())
                .filter_map(|card| u32::try_from(card.catalog_id).ok())
                .collect();
            let names = store::overlay::card_meta(conn, &wanted)?;
            for list in held {
                let mut entries = Vec::new();
                for card in list.cards {
                    let Ok(catalog_id) = u32::try_from(card.catalog_id) else {
                        continue;
                    };
                    ids.push(catalog_id);
                    entries.push(ListEntry {
                        catalog_id: Some(catalog_id),
                        name: overlay::display_name(&names, catalog_id),
                        card_type: names
                            .get(catalog_id)
                            .and_then(|info| info.card_type.clone()),
                        quantity: card.quantity,
                        sideboard: card.sideboard,
                    });
                }
                lists.push(entries);
            }
        }
        (None, None) => return Ok(None),
    }
    ids.sort_unstable();
    ids.dedup();
    let cards = store::overlay::card_meta(conn, &ids)?;
    Ok(Some(overlay::potential_cards(&lists, &revealed, &cards)))
}

/// The sideboard guide of the deck against the archetype (spec 7.6): the
/// match's deck version's zones, the deck's plan, and the numbers over
/// every version of the deck against the archetype.
fn sideboard_guide(
    conn: &Connection,
    deck_id: i64,
    version_id: i64,
    archetype_id: i64,
) -> Result<SideboardGuide, StoreError> {
    let version = store::overlay::deck_version_cards(conn, version_id)?.unwrap_or_default();
    let ids: Vec<u32> = version.iter().map(|card| card.catalog_id).collect();
    let printings = store::overlay::card_meta(conn, &ids)?;
    let version_cards: Vec<VersionCard> = version
        .iter()
        .map(|card| VersionCard {
            catalog_id: card.catalog_id,
            oracle_id: printings.oracle_id(card.catalog_id).map(str::to_string),
            quantity: card.quantity,
            sideboard: card.sideboard,
        })
        .collect();
    let zones = archetypes::guides::zone_quantities(&version_cards);
    let plan = store::overlay::guide_plan(conn, deck_id, archetype_id)?;
    let (sided, postboard) = store::card_stats::sided_stats(conn, deck_id, archetype_id)?;
    let stats = GuideStats {
        cards: sided,
        postboard_games: postboard.games,
        postboard_wins: postboard.wins,
    };

    // One card row per oracle id: the version's own printing first, else
    // the newest printing held (a planned card no longer in the version).
    let mut by_oracle: BTreeMap<String, CardInfo> = BTreeMap::new();
    for card in &version {
        if let Some(info) = printings.get(card.catalog_id)
            && let Some(oracle_id) = &info.oracle_id
        {
            by_oracle
                .entry(oracle_id.clone())
                .or_insert_with(|| info.clone());
        }
    }
    let missing: Vec<String> = plan
        .iter()
        .map(|card| card.oracle_id.clone())
        .filter(|oracle_id| !by_oracle.contains_key(oracle_id))
        .collect();
    for (oracle_id, info) in store::overlay::cards_by_oracle(conn, &missing)? {
        by_oracle.insert(oracle_id, info);
    }
    Ok(overlay::sideboard_rows(
        &zones,
        Some(&plan),
        &stats,
        &by_oracle,
    ))
}

/// The matchup's notes, this deck's and the others' (Plan T `notes_for`).
fn notes(conn: &Connection, deck_id: i64, archetype_id: i64) -> Result<OverlayNotes, StoreError> {
    let notes = store::guides::archetype_notes(conn, deck_id, archetype_id)?;
    let view = |note: archetypes::guides::Note| OverlayNote {
        id: note.id,
        deck_name: note.deck_name,
        body: note.body,
        created_at_ms: note.created_at,
    };
    Ok(OverlayNotes {
        current: notes.current.into_iter().map(view).collect(),
        other: notes.other.into_iter().map(view).collect(),
    })
}

/// The deck the draw odds count (spec 7.4): the current game's first
/// `deck_used` list, split into main and side by the game's first frame
/// (0.x `CreateGames::buildLocalDeck`: after sideboarding the list can repeat
/// game 1's), else the match's deck version (main and sideboard rows; the
/// odds leave the sideboard out).
fn current_deck(
    conn: &Connection,
    inputs: &LiveGameInputs,
    current: Option<&LiveGame>,
) -> Result<Vec<DeckCard>, StoreError> {
    if let Some(game) = current
        && let Some(list) = &game.deck_used
        && !list.is_empty()
    {
        let first_frame = game
            .first_frame
            .as_deref()
            .and_then(game_detail::parse_frame);
        let seat = first_frame
            .as_ref()
            .and_then(|frame| game_detail::seats(frame, &inputs.local_username));
        return Ok(match (&first_frame, seat) {
            (Some(frame), Some((local, _))) => game_detail::local_deck(list, frame, local),
            _ => list.clone(),
        });
    }
    match inputs.deck_version_id {
        Some(version_id) => {
            Ok(store::overlay::deck_version_cards(conn, version_id)?.unwrap_or_default())
        }
        None => Ok(Vec::new()),
    }
}

/// Card rows for the deck and every card of every frame, so a back face or
/// another printing matches by oracle id or face name (U1).
fn card_meta(
    conn: &Connection,
    deck: &[DeckCard],
    frames: &[Option<Frame>],
) -> Result<CardMeta, StoreError> {
    let mut ids: Vec<u32> = deck.iter().map(|card| card.catalog_id).collect();
    for frame in frames.iter().flatten() {
        ids.extend(frame.cards.iter().map(|card| card.catalog_id));
    }
    ids.sort_unstable();
    ids.dedup();
    store::overlay::card_meta(conn, &ids)
}

fn draw_odds(
    deck: &[DeckCard],
    frame: Option<&Frame>,
    local_username: &str,
    cards: &CardMeta,
) -> Option<DrawOdds> {
    // Without the local seat the frame cannot be read: the whole maindeck
    // counts as in the library.
    let seat = frame.and_then(|frame| game_detail::seats(frame, local_username));
    match (frame, seat) {
        (Some(frame), Some((local, _))) => overlay::draw_odds(deck, Some(frame), local, cards),
        _ => overlay::draw_odds(deck, None, 0, cards),
    }
}

/// The current game's strip above the opponent's name.
fn current_game(game: &LiveGame) -> OverlayGame {
    let hand = |mulligans: Option<u32>| mulligans.map(|count| 7u32.saturating_sub(count));
    OverlayGame {
        on_play: game.on_play,
        local_hand: hand(game.local_mulligans),
        opponent_hand: hand(game.opp_mulligans),
    }
}

/// Each game's newest state frame, parsed.
fn newest_frames(live: &LiveGameInputs) -> Vec<Option<Frame>> {
    live.games
        .iter()
        .map(|game| {
            game.newest_frame
                .as_deref()
                .and_then(game_detail::parse_frame)
        })
        .collect()
}

/// The opponent's cards of each game's newest frame, summed across the
/// match and capped at 4 (spec 7.5).
fn revealed_cards(frames: &[Option<Frame>], local_username: &str) -> Vec<(u32, u32)> {
    let mut per_game: Vec<Vec<(u32, u32)>> = Vec::new();
    for frame in frames.iter().flatten() {
        if let Some((_, opponent)) = game_detail::seats(frame, local_username) {
            per_game.push(game_detail::opponent_cards(frame, opponent));
        }
    }
    overlay::aggregate_reveals(&per_game)
}

#[cfg(test)]
mod speed;
#[cfg(test)]
pub(crate) mod tests;
