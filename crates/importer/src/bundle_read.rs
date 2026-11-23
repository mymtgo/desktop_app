//! Bundles pulled from the cloud (Plan Q task 11, 07 spec section 10) read
//! into what the 0.x import takes: a match bundle (0.x's `game_player` JSON,
//! timelines and card stats) becomes an `ImportedMatch` with its
//! `ImportedDetail`, a league bundle an `ImportedLeague` with its drafts and
//! registered decks. Like 0.x's `MatchBundleImporter` and
//! `LeagueBundleImporter`, but through v1's own import paths, so a pulled row
//! is stored exactly as a 0.x one is (live wins).
//!
//! Values are read loosely, as the 0.x readers read theirs: a value v1 cannot
//! read counts as not recorded. Only a bundle without its `match` (or
//! `league`), token or start is refused.

use std::collections::BTreeMap;

use replay::{FrameText, drop_repeats, normalise_time};
use serde_json::Value;
use store::import::{ImportedGame, ImportedMatch, PulledMatch};
use store::import_leagues::{ImportedLeague, LeagueDeck};
use store::import_limited::{LegacyDraft, LegacyLimitedDeck};
use store::imported_detail::{ImportedCardStat, ImportedDetail, ImportedGameDetail, ImportedHand};
use store::sync::{PulledLeague, pulled_legacy_id};
use sync_bundles::league::{DraftInput, LeagueBundleInput};
use sync_bundles::loose::{flag, int, text};
use sync_bundles::match_bundle::{ArchetypeSide, ArenaPlayerFields, CAST_METHODS};
use sync_bundles::{
    BundleError, BundlePlatform, Carry, MATCH_CARRY, match_game_structure, parse_bundle_datetime,
    read_sources,
};

use crate::detail_read::cards;
use crate::endstep_manual::NumberedCards;
use crate::manual_read::{game_detail, read_hand};
use crate::parity_rules::{
    DEFAULT_PACK_SIZE, DEFAULT_PICKS_EXPECTED, DEFAULT_SEAT_COUNT, FINISHED_STATES, OUTCOMES,
    draft_state, known_code, large, main_copies, small,
};

fn missing(part: &str) -> BundleError {
    BundleError::Missing(part.to_string())
}

fn count(value: &Value) -> Option<u32> {
    int(value).and_then(|number| u32::try_from(number).ok())
}

fn time(value: &Value) -> Option<i64> {
    value.as_str().and_then(parse_bundle_datetime)
}

/// A `game_player` JSON value as the text 0.x stored (null is none).
fn json_text(value: &Value) -> Option<String> {
    (!value.is_null()).then(|| value.to_string())
}

/// One game's two seats: the first local and the first other player.
#[derive(Default)]
struct Seats<'a> {
    local: Option<&'a Value>,
    opponent: Option<&'a Value>,
}

fn seats_by_game(bundle: &Value) -> BTreeMap<String, Seats<'_>> {
    let mut seats: BTreeMap<String, Seats> = BTreeMap::new();
    for player in bundle["players"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(game) = text(&player["game"]) else {
            continue;
        };
        if text(&player["username"]).is_none_or(|name| name.trim().is_empty()) {
            continue;
        }
        let entry = seats.entry(game).or_default();
        let slot = if flag(&player["is_local"]) {
            &mut entry.local
        } else {
            &mut entry.opponent
        };
        if slot.is_none() {
            *slot = Some(player);
        }
    }
    seats
}

/// A seat's Arena fields, each read only when it is text.
fn arena_fields(player: Option<&Value>) -> ArenaPlayerFields {
    let Some(player) = player else {
        return ArenaPlayerFields::default();
    };
    ArenaPlayerFields {
        display_name: text(&player["display_name"]),
        arena_user_id: text(&player["arena_user_id"]),
    }
}

/// One `card_stats` row (0.x `card_game_stats`).
fn card_stat(row: &Value) -> ImportedCardStat {
    let number = |key: &str| count(&row[key]).unwrap_or(0);
    let mut methods = Vec::new();
    for method in CAST_METHODS {
        let found = number(method);
        if found > 0 {
            methods.push((method.to_string(), found));
        }
    }
    ImportedCardStat {
        oracle_id: text(&row["oracle_id"])
            .unwrap_or_default()
            .trim()
            .to_string(),
        opponent: flag(&row["opponent"]),
        quantity: number("quantity"),
        kept: number("kept"),
        seen: number("seen"),
        sided_in: flag(&row["sided_in"]),
        sided_out: flag(&row["sided_out"]),
        cast: number("cast"),
        played: count(&row["played"]),
        activated: count(&row["activated"]),
        pregame_revealed: flag(&row["pregame_revealed"]),
        pregame_played: flag(&row["pregame_played"]),
        methods,
        ..ImportedCardStat::default()
    }
}

/// A pulled match bundle as the 0.x import takes it, with its archetype
/// seats and its carry (empty for matches since #9). Games keep the
/// bundle's order (by game id) and are numbered from it.
pub fn read_match_bundle(bundle: &Value) -> Result<(PulledMatch, Carry), BundleError> {
    let section = bundle
        .get("match")
        .filter(|section| section.is_object())
        .ok_or_else(|| missing("match"))?;
    let token = text(&section["token"])
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| missing("match.token"))?;
    let started_at_ms = time(&section["started_at"]).ok_or_else(|| missing("match.started_at"))?;
    let state = text(&section["state"])
        .filter(|state| FINISHED_STATES.contains(&state.as_str()))
        .ok_or_else(|| BundleError::Invalid("match.state".to_string()))?;
    // Absent from bundles built before canonical version 3.
    let manual = flag(&section["manual"]);
    let platform = BundlePlatform::of_bundle(bundle)?;
    let sources = read_sources(bundle, platform)?;
    let seats = seats_by_game(bundle);
    let mut local_player_fields = ArenaPlayerFields::default();
    let mut opponent_player_fields = ArenaPlayerFields::default();
    // An Endstep manual match names its cards by ref: numbered here, mapped
    // to this device's catalog ids by the store.
    let endstep_manual = manual && platform == BundlePlatform::Endstep;
    let mut numbered = NumberedCards::default();
    let signature = text(&section["deck_version_signature"]).unwrap_or_default();
    let registered = if endstep_manual {
        numbered.registered(&signature)
    } else {
        main_copies(&signature)
    };

    let mut local_username: Option<String> = None;
    let mut opponent: Option<String> = None;
    let mut games = Vec::new();
    let mut details = Vec::new();
    let mut manual_details = Vec::new();
    let listed = bundle["games"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    for (index, game) in listed.iter().enumerate() {
        let game_number = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let key = text(&game["mtgo_id"]).unwrap_or_default();
        let empty = Seats::default();
        let seat = seats.get(&key).unwrap_or(&empty);
        if local_username.is_none() {
            local_username = seat.local.and_then(|player| text(&player["username"]));
            local_player_fields = arena_fields(seat.local);
        }
        if opponent.is_none() {
            opponent = seat
                .opponent
                .and_then(|player| text(&player["username"]))
                .filter(|name| Some(name) != local_username.as_ref());
            if opponent.is_some() {
                opponent_player_fields = arena_fields(seat.opponent);
            }
        }
        let local = seat.local.unwrap_or(&Value::Null);
        let other = seat.opponent.unwrap_or(&Value::Null);
        let result = match &game["won"] {
            Value::Bool(true) => "win",
            Value::Bool(false) => "loss",
            _ => "unknown",
        };
        games.push(ImportedGame {
            mtgo_game_id: key.trim().parse().ok(),
            game_number,
            result: result.to_string(),
            on_play: seat.local.map(|player| flag(&player["on_play"])),
            local_mulligans: int(&local["mulligan_count"]),
            opp_mulligans: int(&other["mulligan_count"]),
            local_die_roll: int(&local["dice_roll"]),
            opp_die_roll: int(&other["dice_roll"]),
            turn_count: int(&game["turn_count"]),
            started_at_ms: time(&game["started_at"]),
            ended_at_ms: time(&game["ended_at"]),
        });
        let (local_deck, opponent_deck, hand) = if endstep_manual {
            (
                json_text(&numbered.deck_json(&local["deck_json"])),
                json_text(&numbered.deck_json(&other["deck_json"])),
                json_text(&numbered.hand_json(&local["opening_hand_json"])),
            )
        } else {
            (
                json_text(&local["deck_json"]),
                json_text(&other["deck_json"]),
                json_text(&local["opening_hand_json"]),
            )
        };
        if manual {
            if let Some(detail) = game_detail(
                game_number,
                hand.as_deref(),
                local_deck.as_deref(),
                opponent_deck.as_deref(),
                Some(&registered),
            ) {
                manual_details.push(detail);
            }
            continue;
        }
        let mut frames: Vec<FrameText> = Vec::new();
        for row in bundle["timelines"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            if text(&row["game"]).as_deref() != Some(key.as_str()) || !row["content"].is_object() {
                continue;
            }
            frames.push(FrameText {
                time: text(&row["timestamp"])
                    .map(|time| normalise_time(&time))
                    .unwrap_or_default(),
                text: row["content"].to_string(),
            });
        }
        let frames = drop_repeats(frames);
        let card_stats = bundle["card_stats"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter(|row| text(&row["game"]).as_deref() == Some(key.as_str()))
            .map(card_stat)
            .collect();
        let size = |player: &Value| count(&player["starting_hand_size"]);
        details.push(ImportedGameDetail {
            mtgo_game_id: key.trim().parse().ok(),
            game_number,
            local_deck: local_deck.as_deref().map(cards).unwrap_or_default(),
            opponent_cards: opponent_deck.as_deref().map(cards).unwrap_or_default(),
            frames,
            local_hand: hand
                .as_deref()
                .and_then(read_hand)
                .map(|hand| ImportedHand {
                    starting: hand.kept,
                    bottomed: hand.bottomed,
                    mulligans: hand.mulligans,
                }),
            local_starting_hand_size: seat.local.and_then(size),
            opp_starting_hand_size: seat.opponent.and_then(size),
            card_stats,
        });
    }
    let outcome = text(&section["outcome"])
        .filter(|outcome| OUTCOMES.contains(&outcome.as_str()))
        .unwrap_or_else(|| "unknown".to_string());
    let (games_won_imported, games_lost_imported) = if games.is_empty() {
        (int(&section["games_won"]), int(&section["games_lost"]))
    } else {
        (None, None)
    };
    let archetypes = archetype_sides(bundle, local_username.as_deref());
    let imported = ImportedMatch {
        token,
        // A manual match has no MTGO match id (0.x gave it `manual_*`).
        mtgo_match_id: if manual {
            None
        } else {
            text(&section["mtgo_id"]).and_then(|id| id.trim().parse().ok())
        },
        // The store fills an unknown local player with this device's account.
        local_username: local_username.unwrap_or_default(),
        opponent,
        format: known_code(text(&section["format"])),
        match_type: known_code(text(&section["match_type"])),
        tournament_event_id: int(&section["tournament_mtgo_event_id"]),
        tournament_round: int(&section["tournament_round"]),
        state,
        outcome,
        started_at_ms,
        ended_at_ms: time(&section["ended_at"]),
        note: text(&section["notes"]),
        games,
        manual: manual.then_some(manual_details),
        games_won_imported,
        games_lost_imported,
        deck_linked: false,
        replay_share: None,
        // The match's exact history flag (issue #313); 0.x and v1 always
        // write it, so a bundle without it is a tracked match.
        legacy_history: flag(&section["imported"]),
    };
    let deck = match (
        text(&section["deck_mtgo_id"]),
        text(&section["deck_version_signature"]),
    ) {
        (Some(deck), Some(signature)) => Some((deck, signature)),
        _ => None,
    };
    let pulled = PulledMatch {
        platform,
        sources,
        local_player_fields,
        opponent_player_fields,
        endstep_ranked: (platform == BundlePlatform::Endstep)
            .then(|| sync_bundles::match_endstep_ranked(bundle))
            .flatten(),
        endstep_manual_cards: numbered.cards,
        game_structure: match_game_structure(bundle),
        imported,
        detail: ImportedDetail {
            games: details,
            log: Vec::new(),
        },
        deck,
        league_client_id: text(&section["league_client_id"]),
        history: flag(&section["imported"]),
        archetypes,
        player: sync_bundles::match_player_fields(bundle),
    };
    Ok((pulled, Carry::from_bundle(bundle, MATCH_CARRY)))
}

/// The bundle's archetype rows as seats (archetypes spec section 14): a row
/// whose username is the local player's is the player's seat, any other the
/// opponent's; without games (no username known) `player_is_player` says.
/// A row without a uuid is left out; an unreadable confidence is 0.
fn archetype_sides(bundle: &Value, local_username: Option<&str>) -> Vec<ArchetypeSide> {
    let mut sides = Vec::new();
    for row in bundle["archetypes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(uuid) = text(&row["uuid"]).filter(|uuid| !uuid.trim().is_empty()) else {
            continue;
        };
        let confidence = match &row["confidence"] {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().parse::<f64>().ok(),
            _ => None,
        }
        .unwrap_or(0.0);
        let opponent = match (local_username, text(&row["player_username"])) {
            (Some(local), Some(name)) => name != local,
            _ => !flag(&row["player_is_player"]),
        };
        sides.push(ArchetypeSide {
            uuid,
            confidence,
            manual: flag(&row["manual"]),
            opponent,
        });
    }
    sides
}

fn legacy_draft(draft: &DraftInput, league_id: i64, course_id: Option<i64>) -> LegacyDraft {
    let mut picks = Vec::new();
    let mut notes = Vec::new();
    for pick in &draft.picks {
        let Some(ordinal) = small(Some(pick.ordinal)).filter(|ordinal| *ordinal > 0) else {
            continue;
        };
        if let Some(note) = pick.note.as_deref().filter(|note| !note.trim().is_empty()) {
            notes.push((ordinal, note.to_string()));
        }
        let cards_available = pick
            .cards_available
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(count)
            .collect();
        picks.push(limited::Pick {
            ordinal,
            pack_number: small(Some(pick.pack_number)).unwrap_or(0),
            pick_number: small(Some(pick.pick_number)).unwrap_or(0),
            pack_id: large(pick.pack_id),
            direction: small(pick.direction),
            cards_available,
            picked_catalog_id: small(pick.picked_catalog_id),
            picked_card_id: large(pick.picked_card_id),
            reservations: pick
                .reservations
                .iter()
                .map(|(catalog_id, at_ms)| limited::Reservation {
                    catalog_id: *catalog_id,
                    at_ms: *at_ms,
                })
                .collect(),
            shown_at_ms: pick.shown_at_ms,
            deadline_at_ms: pick.deadline_at_ms,
            picked_at_ms: pick.picked_at_ms,
        });
    }
    picks.sort_by_key(|pick| pick.ordinal);
    LegacyDraft {
        legacy_id: pulled_legacy_id(&draft.draft_token),
        league_id: Some(league_id),
        draft: limited::Draft {
            token: draft.draft_token.clone(),
            league_event_id: None,
            course_id: large(course_id),
            draft_id: large(draft.mtgo_draft_id),
            pod_token: draft.pod_token.clone(),
            seat_index: small(draft.seat_index),
            seat_count: small(Some(draft.seat_count)).unwrap_or(DEFAULT_SEAT_COUNT),
            booster_catalog_id: small(draft.booster_catalog_id),
            state: draft_state(&draft.state).unwrap_or(limited::DraftState::Finished),
            pack_size: small(Some(draft.pack_size)).unwrap_or(DEFAULT_PACK_SIZE),
            picks_expected: small(Some(draft.picks_expected)).unwrap_or(DEFAULT_PICKS_EXPECTED),
            started_at_ms: draft.started_at_ms,
            ended_at_ms: draft.ended_at_ms,
            picks,
        },
        notes,
    }
}

/// A registered deck's cards (`[{catalog_id, quantity, sideboard}]`).
fn registered_cards(cards: &Value) -> Vec<limited::RegisteredCard> {
    let mut found = Vec::new();
    for card in cards.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let (Some(catalog_id), Some(quantity)) =
            (count(&card["catalog_id"]), count(&card["quantity"]))
        else {
            continue;
        };
        if quantity > 0 {
            found.push(limited::RegisteredCard {
                catalog_id,
                quantity,
                sideboard: flag(&card["sideboard"]),
            });
        }
    }
    found
}

/// A pulled league (`read_league_bundle`) as the 0.x league and limited
/// imports take it, under the synthetic 0.x id of its client id.
pub fn pulled_league(input: &LeagueBundleInput, client_id: &str) -> PulledLeague {
    let legacy_id = pulled_legacy_id(client_id);
    let deck = match (&input.deck_mtgo_id, &input.deck_version_signature) {
        (Some(deck), Some(signature)) => Some(LeagueDeck {
            deck_mtgo_id: deck.clone(),
            signature: signature.clone(),
        }),
        _ => None,
    };
    let kind = match input.kind.as_str() {
        "draft" | "sealed" => input.kind.clone(),
        _ => "constructed".to_string(),
    };
    let league = ImportedLeague {
        legacy_id,
        manual: input.manual,
        kind,
        set_code: input.set_code.clone(),
        course_id: input.mtgo_course_id,
        token: input.token.clone(),
        event_id: None,
        name: input.name.clone(),
        format: input.format.clone().unwrap_or_default(),
        state: input.state.clone(),
        started_at_ms: input.started_at_ms,
        joined_at_ms: input.joined_at_ms,
        completed_at_ms: input.completed_at_ms,
        dropped_at_ms: input.dropped_at_ms,
        created_at_ms: None,
        deck,
        notes: input.notes.clone(),
        matches: Vec::new(),
    };
    let drafts = input
        .drafts
        .iter()
        .map(|draft| legacy_draft(draft, legacy_id, input.mtgo_course_id))
        .collect();
    let mut decks = Vec::new();
    for snapshot in &input.snapshots {
        let Some(match_token) = snapshot.match_token.clone() else {
            continue;
        };
        let cards = registered_cards(&snapshot.cards);
        if cards.is_empty() {
            continue;
        }
        decks.push(LegacyLimitedDeck {
            league_id: legacy_id,
            match_token,
            mtgo_match_id: None,
            cards,
            captured_at_ms: snapshot.captured_at_ms,
        });
    }
    PulledLeague {
        league,
        drafts,
        decks,
        player: sync_bundles::PulledLeagueFields {
            removed_at_ms: input.removed_at_ms,
            user_state: input.user_state.clone(),
            user_dropped_at_ms: input.user_dropped_at_ms,
            edited_at_ms: input.edited_at_ms,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use sync_bundles::{ArenaSources, MatchSources};

    use super::*;

    fn arena_bundle() -> Value {
        json!({
            "platform": "arena",
            "match": {
                "token": "t-arena",
                "started_at": "2026-11-01T10:00:00Z",
                "state": "complete",
                "outcome": "win",
            },
            "games": [{ "mtgo_id": "t-arena-1", "won": true }],
            "players": [
                { "game": "t-arena-1", "username": "Fa#11111", "is_local": true,
                  "display_name": "Fa", "arena_user_id": "AAA" },
                { "game": "t-arena-1", "username": "Fb#22222", "is_local": false,
                  "display_name": "Fb" },
            ],
            "sources": { "event_id": "Play_Brawl_Historic", "games": [] },
        })
    }

    #[test]
    fn read_match_bundle_reads_platform_and_sources() {
        let (pulled, _) = read_match_bundle(&arena_bundle()).expect("reads");
        assert_eq!(pulled.platform, BundlePlatform::Arena);
        assert_eq!(
            pulled.sources,
            Some(MatchSources::Arena(ArenaSources {
                event_id: Some("Play_Brawl_Historic".to_string()),
                games: Vec::new(),
            }))
        );
        assert_eq!(
            pulled.local_player_fields.display_name.as_deref(),
            Some("Fa")
        );
        assert_eq!(
            pulled.local_player_fields.arena_user_id.as_deref(),
            Some("AAA")
        );
        assert_eq!(
            pulled.opponent_player_fields.display_name.as_deref(),
            Some("Fb")
        );
        assert_eq!(pulled.opponent_player_fields.arena_user_id, None);
    }

    #[test]
    fn a_non_mtgo_bundle_brings_its_game_structure() {
        let mut bundle = arena_bundle();
        bundle["match"]["game_structure"] = json!("bo1");
        let (pulled, _) = read_match_bundle(&bundle).expect("reads");
        assert_eq!(pulled.game_structure.as_deref(), Some("bo1"));
        bundle.as_object_mut().expect("object").remove("platform");
        bundle.as_object_mut().expect("object").remove("sources");
        let (pulled, _) = read_match_bundle(&bundle).expect("reads");
        assert_eq!(pulled.game_structure, None, "an MTGO bundle never sets it");
    }

    #[test]
    fn a_bundle_without_platform_is_mtgo_and_an_unknown_one_is_refused() {
        let mut bundle = arena_bundle();
        bundle.as_object_mut().expect("object").remove("platform");
        let (pulled, _) = read_match_bundle(&bundle).expect("reads");
        assert_eq!(pulled.platform, BundlePlatform::Mtgo);
        assert_eq!(pulled.sources, None);
        bundle["platform"] = json!("mtga");
        assert!(read_match_bundle(&bundle).is_err());
    }

    #[test]
    fn an_endstep_bundle_carries_its_ranked_result_and_others_none() {
        let mut bundle = arena_bundle();
        bundle["match"]["endstep_ranked"] = json!({ "status": "rated", "rating_after": 1510 });
        let (pulled, _) = read_match_bundle(&bundle).expect("reads");
        assert_eq!(
            pulled.endstep_ranked, None,
            "only an Endstep bundle has one"
        );
        bundle["platform"] = json!("endstep");
        bundle["sources"] = json!({ "forge_game_id": "q-game", "units": [] });
        let (pulled, _) = read_match_bundle(&bundle).expect("reads");
        let ranked = pulled.endstep_ranked.expect("the ranked result");
        assert_eq!(ranked.status.as_deref(), Some("rated"));
        assert_eq!(ranked.rating_after, Some(1510));
        assert_eq!(ranked.delta, None);
    }
}
