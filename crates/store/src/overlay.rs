//! Reads for the overlays (overlay spec sections 7, 9 and 10): a live match's
//! games with their newest frames and deck lists, its sideboarding state
//! changes, the head-to-head record against an opponent, the league card's
//! runs, the last played deck, the current login and card rows.
//!
//! Read-only: the overlay threads call these on their own reader connection.
//! The rules applied to what is read (draw odds, reveals, sideboarding, the
//! league card) are in the pure `overlay` crate.

use decks::DeckCard;
use std::collections::HashMap;

use overlay::{
    CardInfo, CardMeta, LeagueCardInput, LeagueMatchInput, LeagueRunInput, OverlayDeck, Position,
    TournamentInput, TournamentMatchInput,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::names::{game_won, stored_outcome, stored_state};
use crate::platform::account_of_mtgo;
use crate::write::decode_raw;

/// One game of a live match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveGame {
    pub number: u32,
    pub mtgo_game_id: Option<u64>,
    /// The game's first `deck_used` list (in log order), as played: main and
    /// sideboard entries.
    pub deck_used: Option<Vec<DeckCard>>,
    /// The raw text of the game's newest `game_state_update` (in log order).
    pub newest_frame: Option<String>,
    /// The log position of the game's first `game_state_update`.
    pub first_frame_at: Option<Position>,
    /// The raw text of the game's first `game_state_update`: its Sideboard
    /// zone tells which `deck_used` cards were sided out.
    pub first_frame: Option<String>,
    /// The local player is on the play (`None` before the choice is logged).
    pub on_play: Option<bool>,
    /// Mulligans of each player, logged once their hand is kept.
    pub local_mulligans: Option<u32>,
    pub opp_mulligans: Option<u32>,
}

/// What the game overlay reads about the live match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveGameInputs {
    pub match_id: i64,
    pub account_id: i64,
    /// The local player's username (to find the local seat in a frame).
    pub local_username: String,
    pub opponent_id: Option<i64>,
    /// The opponent's username (`None` before one is known).
    pub opponent_username: Option<String>,
    pub format: Option<String>,
    /// The deck version the match is linked to.
    pub deck_version_id: Option<i64>,
    /// The deck of that version.
    pub deck_id: Option<i64>,
    /// Every game, by game number.
    pub games: Vec<LiveGame>,
}

/// The live match's games and what the game overlay reads from them (spec
/// 7.4, 7.5): per game its first deck list, newest frame and first frame
/// position. `None` when the token has no `matches` row. The caller picks
/// the current game's deck list, else the linked deck version (spec 7.4).
pub fn live_game_inputs(
    conn: &Connection,
    match_token: &str,
) -> Result<Option<LiveGameInputs>, StoreError> {
    let head = conn
        .prepare_cached(
            "SELECT m.id, m.account_id, a.username, m.opponent_id, o.username, m.format,
                 m.deck_version_id, v.deck_id
             FROM matches m JOIN accounts a ON a.id = m.account_id
                 LEFT JOIN opponents o ON o.id = m.opponent_id
                 LEFT JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE m.token = ?1",
        )?
        .query_row([match_token], |row| {
            Ok(LiveGameInputs {
                match_id: row.get(0)?,
                account_id: row.get(1)?,
                local_username: row.get(2)?,
                opponent_id: row.get(3)?,
                opponent_username: row.get(4)?,
                format: row.get(5)?,
                deck_version_id: row.get(6)?,
                deck_id: row.get(7)?,
                games: Vec::new(),
            })
        })
        .optional()?;
    let Some(mut inputs) = head else {
        return Ok(None);
    };

    let mut games: Vec<LiveGame> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT game_number, mtgo_game_id, on_play, local_mulligans, opp_mulligans
             FROM games WHERE match_id = ?1 ORDER BY game_number",
        )?;
        let rows = stmt.query_map([inputs.match_id], |row| {
            let id: Option<i64> = row.get(1)?;
            let count = |value: Option<i64>| value.and_then(|value| u32::try_from(value).ok());
            Ok(LiveGame {
                number: row.get(0)?,
                mtgo_game_id: id.and_then(|id| u64::try_from(id).ok()),
                deck_used: None,
                newest_frame: None,
                first_frame_at: None,
                first_frame: None,
                on_play: row.get(2)?,
                local_mulligans: count(row.get(3)?),
                opp_mulligans: count(row.get(4)?),
            })
        })?;
        for row in rows {
            games.push(row?);
        }
    }

    for mut game in games {
        let mtgo_game_id = game.mtgo_game_id;
        if let Some(game_id) = mtgo_game_id {
            game.deck_used = crate::decks::deck_used_raw(conn, game_id)?
                .and_then(|raw| decks::parse_deck_used(&raw))
                .map(|(_, cards)| cards);
            game.newest_frame = latest_frame(conn, game_id)?;
            game.first_frame_at = first_frame_at(conn, game_id)?;
            game.first_frame = first_frame(conn, game_id)?;
        }
        inputs.games.push(game);
    }
    Ok(Some(inputs))
}

/// The newest state frame of a game, in log order: one seek of
/// `log_events_frames` (spec 14, "newest frame read < 1 ms").
pub(crate) const LATEST_FRAME: &str = "SELECT raw, compressed FROM log_events
     WHERE kind = 'game_state_update' AND game_id = ?1 AND quarantined = 0
     ORDER BY log_file_id DESC, byte_offset DESC LIMIT 1";

/// The first state frame of a game, in log order, through the same index.
pub(crate) const FIRST_FRAME: &str = "SELECT log_file_id, byte_offset FROM log_events
     WHERE kind = 'game_state_update' AND game_id = ?1 AND quarantined = 0
     ORDER BY log_file_id, byte_offset LIMIT 1";

/// The first state frame's text, through the same index.
pub(crate) const FIRST_FRAME_RAW: &str = "SELECT raw, compressed FROM log_events
     WHERE kind = 'game_state_update' AND game_id = ?1 AND quarantined = 0
     ORDER BY log_file_id, byte_offset LIMIT 1";

/// The raw text of a game's newest `game_state_update` (in log order,
/// `(log_file_id, byte_offset)`), decoded. A compacted match's frames are in
/// its archive, not here: a live game is never compacted.
pub fn latest_frame(conn: &Connection, mtgo_game_id: u64) -> Result<Option<String>, StoreError> {
    let row = conn
        .prepare_cached(LATEST_FRAME)?
        .query_row([mtgo_game_id as i64], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
        })
        .optional()?;
    match row {
        Some((raw, compressed)) => Ok(Some(decode_raw(raw, compressed)?)),
        None => Ok(None),
    }
}

/// The raw text of a game's first `game_state_update` (in log order),
/// decoded.
pub fn first_frame(conn: &Connection, mtgo_game_id: u64) -> Result<Option<String>, StoreError> {
    let row = conn
        .prepare_cached(FIRST_FRAME_RAW)?
        .query_row([mtgo_game_id as i64], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
        })
        .optional()?;
    match row {
        Some((raw, compressed)) => Ok(Some(decode_raw(raw, compressed)?)),
        None => Ok(None),
    }
}

/// The log position of a game's first `game_state_update`: where the game
/// starts for the sideboarding check (spec 7.7).
pub fn first_frame_at(
    conn: &Connection,
    mtgo_game_id: u64,
) -> Result<Option<Position>, StoreError> {
    Ok(conn
        .prepare_cached(FIRST_FRAME)?
        .query_row([mtgo_game_id as i64], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

/// The match's `match_state_changed` rows whose text names a sideboarding
/// state, as `(position, text)` in log order (spec 7.7): the input of
/// `overlay::is_sideboarding`. A compressed row cannot be searched in SQL,
/// so it is decoded and checked here.
pub fn sideboarding_events(
    conn: &Connection,
    match_token: &str,
) -> Result<Vec<(Position, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT log_file_id, byte_offset, raw, compressed FROM log_events
         WHERE match_token = ?1 AND kind = 'match_state_changed' AND quarantined = 0
           AND (compressed = 1 OR instr(raw, 'SideboardingState') > 0)
         ORDER BY log_file_id, byte_offset",
    )?;
    let rows = stmt.query_map([match_token], |row| {
        Ok((
            (row.get(0)?, row.get(1)?),
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (position, raw, compressed) = row?;
        let text = decode_raw(raw, compressed)?;
        if text.contains(SIDEBOARDING_STATE) {
            events.push((position, text));
        }
    }
    Ok(events)
}

const SIDEBOARDING_STATE: &str = "SideboardingState";

/// The head-to-head record against an opponent (0.x `ResolveOverlayOpponent::
/// headToHead`, spec 7.3): `(wins, losses)` over complete, listed matches
/// against them in the same format (no format meets no format), the live
/// match left out. Draws count in neither.
pub fn head_to_head(
    conn: &Connection,
    opponent_id: i64,
    format: Option<&str>,
    exclude_match_id: i64,
) -> Result<(u32, u32), StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT COALESCE(SUM(outcome = 'win'), 0), COALESCE(SUM(outcome = 'loss'), 0)
             FROM listed_matches
             WHERE opponent_id = ?1 AND format IS ?2 AND id <> ?3 AND state = 'complete'",
        )?
        .query_row(params![opponent_id, format, exclude_match_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?)
}

/// The runs that can be on the league card, as a common table expression
/// over `?1` (the account) and `?2` (the grace cutoff): active runs with a
/// listed match, and complete or dropped ones with listed matches that ended
/// at or after the cutoff (0.x `BuildOverlayState::selectLeague`, before its
/// ordering, which the pure card rules apply). `deck_ref` is the run's deck:
/// its own, else its version's, else its first non-abandoned versioned
/// match's (0.x `deckFor`). A run the player deleted is never on the card.
///
/// A status the player set that ends a run (Complete, Partial; Dropped is
/// the fold's own drop) takes it off the card as on the Leagues page; one
/// that makes it Active does not, so marking an old run Active never makes
/// it the current one. A run the player marked Complete that the fold has
/// not completed ended when it was marked (`edited_at`, written only by the
/// status, drop, delete and restore edits: restoring a deleted run starts
/// its finished time again). A player's drop the fold has not taken yet
/// ended at the drop (`user_dropped_at`).
const CARD_RUNS: &str = "WITH shown AS (
    SELECT l.*,
        CASE WHEN l.user_state IN ('complete', 'partial', 'dropped') THEN l.user_state
             ELSE l.state END AS shown_state,
        CASE WHEN l.user_state = 'complete' THEN COALESCE(l.completed_at, l.edited_at)
             ELSE l.completed_at END AS shown_completed_at,
        CASE WHEN l.user_state = 'dropped' THEN COALESCE(l.dropped_at, l.user_dropped_at)
             ELSE l.dropped_at END AS shown_dropped_at
    FROM listed_leagues l
), runs AS (
    SELECT l.id, l.kind, l.rounds, l.shown_state AS state, l.name, l.format, l.started_at,
        l.shown_completed_at AS completed_at, l.shown_dropped_at AS dropped_at,
        COALESCE(MAX(l.completed_at, l.dropped_at), l.completed_at, l.dropped_at)
            AS fold_ended_at,
        COALESCE(
            l.deck_id,
            (SELECT v.deck_id FROM deck_versions v WHERE v.id = l.deck_version_id),
            (SELECT v.deck_id FROM listed_matches m
                 JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE m.league_id = l.id AND m.state <> 'abandoned'
             ORDER BY m.started_at, m.id LIMIT 1)
        ) AS deck_ref
    FROM shown l
    WHERE l.account_id = ?1 AND l.detached_at IS NULL
      AND EXISTS (SELECT 1 FROM listed_matches m WHERE m.league_id = l.id)
      AND (l.shown_state = 'active'
           OR (l.shown_state IN ('complete', 'dropped')
               AND (l.shown_completed_at >= ?2 OR l.shown_dropped_at >= ?2)))
)";

/// Every match on the card's events, after `CARD_RUNS`, over `?3` too (the
/// account's candidate tournaments, a JSON array of MTGO event ids): a run's
/// listed matches by `league_id`, and a tournament's by the account and
/// `tournament_event_id` (never `tournament_id`, which the league sync sets
/// late). `owner_kind` and `owner_id` say whose match it is.
const CARD_MATCHES: &str = ", card_matches AS (
    SELECT 'league' AS owner_kind, r.id AS owner_id, m.id, m.token, m.state, m.outcome,
        m.started_at, m.ended_at, m.deck_version_id, m.tournament_round
    FROM runs r JOIN listed_matches m ON m.league_id = r.id
    UNION ALL
    SELECT 'tournament', m.tournament_event_id, m.id, m.token, m.state, m.outcome,
        m.started_at, m.ended_at, m.deck_version_id, m.tournament_round
    FROM listed_matches m
    WHERE m.account_id = ?1 AND m.tournament_event_id IN (SELECT value FROM json_each(?3))
)";

/// The deck columns `deck_row` reads, of `decks d` and its cover `cards c`.
const DECK_COLUMNS: &str = "d.id, d.name, d.color_identity, d.overlay_label, c.art_crop,
    c.local_art_crop, (SELECT NULLIF(ar.name, ar.uuid) FROM archetypes ar
        WHERE ar.id = d.archetype_id), d.cover_file";

fn deck_row(row: &rusqlite::Row, first: usize) -> rusqlite::Result<OverlayDeck> {
    Ok(OverlayDeck {
        id: row.get(first)?,
        name: row.get(first + 1)?,
        // Plan T's `decks.archetype_id` (null before detection).
        archetype: row.get(first + 6)?,
        color_identity: row.get(first + 2)?,
        label: row.get(first + 3)?,
        cover_art_crop: row.get(first + 4)?,
        cover_local_art_crop: row.get(first + 5)?,
        cover_file: row.get(first + 7)?,
    })
}

/// What the league card is built from (spec 9.1): the account's candidate
/// runs with their listed matches, games, decks and, for an in-progress
/// match, its sideboarding inputs (and, for the one the card shows, its live
/// frames and the username), plus the last played deck. A finished run is read
/// while it ended within `finished_ms` of `now_ms` (the
/// `overlay_finished_minutes` setting). The account's stored tournaments
/// that are not stale come with their matches and decks (event overlay spec
/// 6). At most eleven queries, however many runs, tournaments and matches
/// (0.x "stays within a query budget while in game"); two more only while a
/// tournament is before or in round 1 (its registered deck), and one more
/// only while a tournament has no deck of its own (its format's last played
/// deck).
pub fn league_card_inputs(
    conn: &Connection,
    account_id: i64,
    now_ms: i64,
    finished_ms: i64,
) -> Result<LeagueCardInput, StoreError> {
    let mut queries = 0;
    league_card_inputs_counted(conn, account_id, now_ms, finished_ms, &mut queries)
}

/// `league_card_inputs`, adding one to `queries` for every query it runs.
fn league_card_inputs_counted(
    conn: &Connection,
    account_id: i64,
    now_ms: i64,
    finished_ms: i64,
    queries: &mut u32,
) -> Result<LeagueCardInput, StoreError> {
    let cutoff = now_ms - finished_ms;
    let mut input = LeagueCardInput::default();

    // 1. The runs, newest first.
    let mut deck_refs: Vec<Option<i64>> = Vec::new();
    {
        *queries += 1;
        let mut stmt = conn.prepare_cached(&format!(
            "{CARD_RUNS}
             SELECT id, kind, state, name, format, started_at, completed_at, dropped_at, deck_ref,
                 rounds, fold_ended_at
             FROM runs ORDER BY started_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(params![account_id, cutoff], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
            ))
        })?;
        for row in rows {
            let (
                id,
                kind,
                state,
                name,
                format,
                started,
                completed,
                dropped,
                deck_ref,
                rounds,
                fold_ended,
            ) = row?;
            input.runs.push(LeagueRunInput {
                id,
                kind: crate::leagues::parse_kind(&kind)?,
                rounds: rounds.map(|rounds| rounds as usize),
                state: crate::leagues::parse_state(&state)?,
                name,
                format,
                started_at_ms: started,
                completed_at_ms: completed,
                dropped_at_ms: dropped,
                fold_ended_at_ms: fold_ended,
                matches: Vec::new(),
                deck: None,
            });
            deck_refs.push(deck_ref);
        }
    }
    // 2. The tournaments that are not stale.
    read_tournaments(conn, account_id, now_ms, &mut input, queries)?;
    if input.runs.is_empty() && input.tournaments.is_empty() {
        *queries += 1;
        input.last_played_deck = last_played_deck(conn)?;
        return Ok(input);
    }
    let event_ids: Vec<i64> = input
        .tournaments
        .iter()
        .map(|tournament| tournament.mtgo_event_id)
        .collect();
    let events = serde_json::to_string(&event_ids)
        .map_err(|error| StoreError::Corrupt(format!("event ids: {error}")))?;
    let registered_refs =
        registered_deck_refs(conn, account_id, now_ms, &input.tournaments, queries)?;

    // 3. Their listed matches, oldest first.
    let mut match_deck_refs: Vec<(i64, i64)> = Vec::new(); // (match id, deck id)
    {
        *queries += 1;
        let mut stmt = conn.prepare_cached(&format!(
            "{CARD_RUNS}{CARD_MATCHES}
             SELECT m.owner_kind, m.owner_id, m.id, m.token, m.state, m.outcome, m.started_at,
                 m.ended_at, m.tournament_round,
                 (SELECT v.deck_id FROM deck_versions v WHERE v.id = m.deck_version_id)
             FROM card_matches m
             ORDER BY m.started_at, m.id"
        ))?;
        let rows = stmt.query_map(params![account_id, cutoff, events], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                LeagueMatchInput {
                    id: row.get(2)?,
                    token: row.get(3)?,
                    state: stored_state(&row.get::<_, String>(4)?),
                    outcome: stored_outcome(&row.get::<_, String>(5)?),
                    started_at_ms: row.get(6)?,
                    ended_at_ms: row.get(7)?,
                    games: Vec::new(),
                    sideboarding_events: Vec::new(),
                    newest_game_start: None,
                    first_frame: None,
                    newest_frame: None,
                },
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
            ))
        })?;
        for row in rows {
            let (owner_kind, owner_id, member, round, deck_ref) = row?;
            if let Some(deck_ref) = deck_ref {
                match_deck_refs.push((member.id, deck_ref));
            }
            if owner_kind == "league" {
                if let Some(run) = input.runs.iter_mut().find(|run| run.id == owner_id) {
                    run.matches.push(member);
                }
            } else if let Some(tournament) = input
                .tournaments
                .iter_mut()
                .find(|tournament| tournament.mtgo_event_id == owner_id)
            {
                tournament.matches.push(TournamentMatchInput {
                    round: round.and_then(|round| u32::try_from(round).ok()),
                    deck: None,
                    member,
                });
            }
        }
    }

    // 4. Their games, by game number.
    {
        *queries += 1;
        let mut stmt = conn.prepare_cached(&format!(
            "{CARD_RUNS}{CARD_MATCHES}
             SELECT g.match_id, g.result
             FROM card_matches m JOIN games g ON g.match_id = m.id
             ORDER BY g.match_id, g.game_number"
        ))?;
        let rows = stmt.query_map(params![account_id, cutoff, events], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (match_id, result) = row?;
            let won = game_won(&result);
            if let Some(member) = member_mut(&mut input, match_id) {
                member.games.push(won);
            }
        }
    }

    // 5. Their decks and cover art: each run's, each tournament match's,
    // each tournament's registered deck, and for a tournament with neither
    // a match deck nor a registered deck, its format's last played deck.
    {
        let deckless: Vec<i64> = input
            .tournaments
            .iter()
            .filter(|tournament| {
                !registered_refs.contains_key(&tournament.mtgo_event_id)
                    && !tournament.matches.iter().any(|played| {
                        match_deck_refs
                            .iter()
                            .any(|(match_id, _)| *match_id == played.member.id)
                    })
            })
            .map(|tournament| tournament.mtgo_event_id)
            .collect();
        let formats: Vec<String> = input
            .tournaments
            .iter()
            .filter(|tournament| deckless.contains(&tournament.mtgo_event_id))
            .filter_map(|tournament| tournament.format.clone())
            .collect::<std::collections::BTreeSet<String>>()
            .into_iter()
            .collect();
        let format_refs = format_deck_refs(conn, account_id, &formats, queries)?;
        let mut ids: Vec<i64> = deck_refs.iter().flatten().copied().collect();
        ids.extend(match_deck_refs.iter().map(|(_, deck)| *deck));
        ids.extend(registered_refs.values().copied());
        ids.extend(format_refs.values().copied());
        ids.sort_unstable();
        ids.dedup();
        let ids = serde_json::to_string(&ids)
            .map_err(|error| StoreError::Corrupt(format!("deck ids: {error}")))?;
        *queries += 1;
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT {DECK_COLUMNS}
             FROM decks d LEFT JOIN cards c ON c.catalog_id = d.cover_catalog_id
             WHERE d.id IN (SELECT value FROM json_each(?1))"
        ))?;
        let rows = stmt.query_map([ids], |row| deck_row(row, 0))?;
        let mut decks: HashMap<i64, OverlayDeck> = HashMap::new();
        for row in rows {
            let deck = row?;
            decks.insert(deck.id, deck);
        }
        let deck_of = |id: Option<i64>| id.and_then(|id| decks.get(&id).cloned());
        for (run, deck_ref) in input.runs.iter_mut().zip(&deck_refs) {
            run.deck = deck_of(*deck_ref);
        }
        for tournament in input.tournaments.iter_mut() {
            let format_ref = tournament
                .format
                .as_ref()
                .filter(|_| deckless.contains(&tournament.mtgo_event_id))
                .and_then(|format| format_refs.get(format).copied());
            tournament.format_deck = deck_of(format_ref);
            tournament.registered_deck =
                deck_of(registered_refs.get(&tournament.mtgo_event_id).copied());
            for played in tournament.matches.iter_mut() {
                let deck_ref = match_deck_refs
                    .iter()
                    .find(|(match_id, _)| *match_id == played.member.id)
                    .map(|(_, deck)| *deck);
                played.deck = deck_of(deck_ref);
            }
        }
    }

    // 6. The sideboarding state changes of their in-progress matches.
    {
        *queries += 1;
        let mut stmt = conn.prepare_cached(&format!(
            "{CARD_RUNS}{CARD_MATCHES}
             SELECT m.id, e.log_file_id, e.byte_offset, e.raw, e.compressed
             FROM card_matches m
                 JOIN log_events e ON e.match_token = m.token
             WHERE m.state = 'in_progress' AND e.kind = 'match_state_changed'
               AND e.quarantined = 0
               AND (e.compressed = 1 OR instr(e.raw, 'SideboardingState') > 0)
             ORDER BY m.id, e.log_file_id, e.byte_offset"
        ))?;
        let rows = stmt.query_map(params![account_id, cutoff, events], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (row.get(1)?, row.get(2)?),
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?;
        for row in rows {
            let (match_id, position, raw, compressed) = row?;
            let text = decode_raw(raw, compressed)?;
            if !text.contains(SIDEBOARDING_STATE) {
                continue;
            }
            if let Some(member) = member_mut(&mut input, match_id) {
                member.sideboarding_events.push((position, text));
            }
        }
    }

    // 7. The first frame of each in-progress match's newest game.
    let mut newest_games: Vec<(i64, i64)> = Vec::new(); // (match_id, mtgo_game_id)
    {
        *queries += 1;
        let mut stmt = conn.prepare_cached(&format!(
            "{CARD_RUNS}{CARD_MATCHES}
             SELECT m.id, e.log_file_id, e.byte_offset, g.mtgo_game_id
             FROM card_matches m
                 JOIN games g ON g.match_id = m.id
                     AND g.game_number = (SELECT MAX(n.game_number) FROM games n
                                          WHERE n.match_id = m.id)
                 JOIN log_events e ON e.rowid = (
                     SELECT f.rowid FROM log_events f
                     WHERE f.kind = 'game_state_update' AND f.game_id = g.mtgo_game_id
                       AND f.quarantined = 0
                     ORDER BY f.log_file_id, f.byte_offset LIMIT 1)
             WHERE m.state = 'in_progress'"
        ))?;
        let rows = stmt.query_map(params![account_id, cutoff, events], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                (row.get(1)?, row.get(2)?),
                row.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (match_id, position, game_id) = row?;
            if let Some(member) = member_mut(&mut input, match_id) {
                member.newest_game_start = Some(position);
                newest_games.push((match_id, game_id));
            }
        }
    }

    // 8. The newest game of the live match the card shows (the overlay's own
    // choice, so the life always belongs to the shown match): its first and
    // newest frames and the local username, for the life bars (themes spec
    // 4.2).
    let shown = overlay::shown_live_match_id(&input, now_ms, finished_ms);
    let shown_game = newest_games
        .iter()
        .find(|(match_id, _)| Some(*match_id) == shown)
        .copied();
    if let Some((match_id, game_id)) = shown_game {
        *queries += 1;
        let first = first_frame(conn, game_id as u64)?;
        *queries += 1;
        let newest = latest_frame(conn, game_id as u64)?;
        *queries += 1;
        input.local_username = conn
            .prepare_cached("SELECT username FROM accounts WHERE id = ?1")?
            .query_row([account_id], |row| row.get(0))
            .optional()?;
        if let Some(member) = member_mut(&mut input, match_id) {
            member.first_frame = first;
            member.newest_frame = newest;
        }
    }

    // 9. The last played deck, for when no event is chosen.
    *queries += 1;
    input.last_played_deck = last_played_deck(conn)?;
    Ok(input)
}

fn member_mut(input: &mut LeagueCardInput, match_id: i64) -> Option<&mut LeagueMatchInput> {
    let runs = input.runs.iter_mut().flat_map(|run| run.matches.iter_mut());
    let tournaments = input.tournaments.iter_mut().flat_map(|tournament| {
        tournament
            .matches
            .iter_mut()
            .map(|played| &mut played.member)
    });
    runs.chain(tournaments).find(|member| member.id == match_id)
}

/// How far back the card reads tournament deck lines (the registered deck):
/// the tournament sync's window, as a stored standing never outlives it.
pub const REGISTERED_DECK_WITHIN_MS: i64 = crate::tournament_sync::WINDOW_MS;

/// One `tournament_standings` row as read for the card, its status, finish
/// and stage still text.
struct StandingRead {
    mtgo_event_id: i64,
    name: Option<String>,
    format: Option<String>,
    status: String,
    finish: Option<String>,
    stage: String,
    round: Option<i64>,
    swiss_rounds: Option<i64>,
    cut_size: Option<i64>,
    rank: Option<i64>,
    players: Option<i64>,
    matches_left: Option<i64>,
    place: Option<i64>,
    byes: i64,
    starts_at: Option<i64>,
    round_result_at: Option<i64>,
    finished_at: Option<i64>,
    last_transition_at: i64,
    last_event_at: i64,
}

fn standing_read(row: &rusqlite::Row) -> rusqlite::Result<StandingRead> {
    Ok(StandingRead {
        mtgo_event_id: row.get(0)?,
        name: row.get(1)?,
        format: row.get(2)?,
        status: row.get(3)?,
        finish: row.get(4)?,
        stage: row.get(5)?,
        round: row.get(6)?,
        swiss_rounds: row.get(7)?,
        cut_size: row.get(8)?,
        rank: row.get(9)?,
        players: row.get(10)?,
        matches_left: row.get(11)?,
        place: row.get(12)?,
        byes: row.get(13)?,
        starts_at: row.get(14)?,
        round_result_at: row.get(15)?,
        finished_at: row.get(16)?,
        last_transition_at: row.get(17)?,
        last_event_at: row.get(18)?,
    })
}

fn count(value: Option<i64>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

fn tournament_input(read: StandingRead) -> Result<TournamentInput, StoreError> {
    Ok(TournamentInput {
        mtgo_event_id: read.mtgo_event_id,
        name: read.name,
        format: read.format,
        status: crate::tournament_sync::parse_status(&read.status)?,
        finish: read
            .finish
            .as_deref()
            .map(crate::tournament_sync::parse_finish)
            .transpose()?,
        round: count(read.round),
        stage: crate::tournament_sync::parse_stage(&read.stage)?,
        swiss_rounds: count(read.swiss_rounds),
        cut_size: count(read.cut_size),
        rank: count(read.rank),
        players: count(read.players),
        matches_left: count(read.matches_left),
        byes: count(Some(read.byes)).unwrap_or(0),
        place: count(read.place),
        starts_at_ms: read.starts_at,
        round_result_at_ms: read.round_result_at,
        finished_at_ms: read.finished_at,
        last_transition_at_ms: read.last_transition_at,
        last_event_at_ms: read.last_event_at,
        matches: Vec::new(),
        registered_deck: None,
        format_deck: None,
    })
}

/// The account's stored tournaments (`tournament_standings`) whose newest
/// fact is within `overlay::STALE_AFTER_MS` of `now_ms`, or that are
/// registered and not yet `overlay::UNPAIRED_AFTER_START_MS` past their
/// start (the card's own rules cut the rest), with no matches or decks yet,
/// into `input`.
fn read_tournaments(
    conn: &Connection,
    account_id: i64,
    now_ms: i64,
    input: &mut LeagueCardInput,
    queries: &mut u32,
) -> Result<(), StoreError> {
    *queries += 1;
    let mut stmt = conn.prepare_cached(
        "SELECT mtgo_event_id, name, format, status, finish, stage, round, swiss_rounds,
             cut_size, rank, players, matches_left, place, byes, starts_at, round_result_at,
             finished_at, last_transition_at, last_event_at
         FROM tournament_standings
         WHERE account_id = ?1
           AND (last_event_at >= ?2 OR (status = 'registered' AND starts_at >= ?3))
         ORDER BY mtgo_event_id",
    )?;
    let stale = now_ms - overlay::STALE_AFTER_MS;
    let unpaired = now_ms - overlay::UNPAIRED_AFTER_START_MS;
    let rows = stmt.query_map(params![account_id, stale, unpaired], standing_read)?;
    for row in rows {
        input.tournaments.push(tournament_input(row?)?);
    }
    Ok(())
}

/// The tournament deck lines logged within `REGISTERED_DECK_WITHIN_MS` of
/// now (the tournament form of `match_deck_registered`, through
/// `log_events_registered`).
const RECENT_TOURNAMENT_DECK_RAWS: &str = "SELECT log_file_id, byte_offset, raw, compressed
     FROM log_events
     WHERE match_token IS NULL AND kind = 'match_deck_registered' AND quarantined = 0
       AND logged_at >= ?1";

/// The account's deck versions with one of these signatures (a JSON array),
/// newest first: through `decks_account_id` and the versions' unique
/// `(deck_id, signature)` index.
const ACCOUNT_VERSIONS_BY_SIGNATURE: &str = "SELECT v.signature, v.deck_id
     FROM decks d JOIN deck_versions v ON v.deck_id = d.id
     WHERE d.account_id = ?1 AND v.signature IN (SELECT value FROM json_each(?2))
     ORDER BY v.modified_at DESC, v.id DESC";

/// The account's deck id registered for each tournament that may show it
/// (before or in round 1: the registered card's deck, D9), by MTGO event id:
/// the first tournament deck line of the event (in log order, logged within
/// the window) whose cards are one of the account's deck versions. Two
/// queries, only when such a tournament is read.
fn registered_deck_refs(
    conn: &Connection,
    account_id: i64,
    now_ms: i64,
    tournaments: &[TournamentInput],
    queries: &mut u32,
) -> Result<HashMap<i64, i64>, StoreError> {
    let mut found = HashMap::new();
    let wanted: Vec<i64> = tournaments
        .iter()
        .filter(|tournament| tournament.round.is_none_or(|round| round <= 1))
        .map(|tournament| tournament.mtgo_event_id)
        .collect();
    if wanted.is_empty() {
        return Ok(found);
    }
    *queries += 1;
    let mut stmt = conn.prepare_cached(RECENT_TOURNAMENT_DECK_RAWS)?;
    let rows = stmt.query_map([now_ms - REGISTERED_DECK_WITHIN_MS], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    // The first list in log order per event.
    let mut firsts: HashMap<i64, ((i64, i64), String)> = HashMap::new();
    for row in rows {
        let (file, offset, raw, compressed) = row?;
        let Some((event, cards)) = decks::parse_tournament_deck(&decode_raw(raw, compressed)?)
        else {
            continue;
        };
        let Ok(event) = i64::try_from(event) else {
            continue;
        };
        if !wanted.contains(&event) || cards.is_empty() {
            continue;
        }
        let earlier = firsts
            .get(&event)
            .is_none_or(|(at, _)| (file, offset) < *at);
        if earlier {
            firsts.insert(event, ((file, offset), decks::signature(&cards)));
        }
    }
    if firsts.is_empty() {
        return Ok(found);
    }
    let signatures: Vec<&String> = firsts.values().map(|(_, signature)| signature).collect();
    let signatures = serde_json::to_string(&signatures)
        .map_err(|error| StoreError::Corrupt(format!("signatures: {error}")))?;
    *queries += 1;
    let mut stmt = conn.prepare_cached(ACCOUNT_VERSIONS_BY_SIGNATURE)?;
    let rows = stmt.query_map(params![account_id, signatures], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut decks_by_signature: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (signature, deck_id) = row?;
        decks_by_signature.entry(signature).or_insert(deck_id);
    }
    for (event, (_, signature)) in firsts {
        if let Some(deck_id) = decks_by_signature.get(&signature) {
            found.insert(event, *deck_id);
        }
    }
    Ok(found)
}

/// The account's last played deck of each of these formats (a JSON array):
/// one seek per format through `matches_account_format_started` (migration
/// 0028), newest first, the first listed match.
const FORMAT_DECKS: &str = "SELECT f.value,
         (SELECT v.deck_id FROM listed_matches m
              JOIN deck_versions v ON v.id = m.deck_version_id
          WHERE m.account_id = ?1 AND m.format = f.value AND m.deck_version_id IS NOT NULL
          ORDER BY m.started_at DESC LIMIT 1)
     FROM json_each(?2) f";

/// The last played deck id of each format, for the tournaments with no
/// deck of their own (no match deck and no registered deck, D9): one query,
/// only when one is needed.
fn format_deck_refs(
    conn: &Connection,
    account_id: i64,
    formats: &[String],
    queries: &mut u32,
) -> Result<HashMap<String, i64>, StoreError> {
    let mut found = HashMap::new();
    if formats.is_empty() {
        return Ok(found);
    }
    let list = serde_json::to_string(formats)
        .map_err(|error| StoreError::Corrupt(format!("formats: {error}")))?;
    *queries += 1;
    let mut stmt = conn.prepare_cached(FORMAT_DECKS)?;
    let rows = stmt.query_map(params![account_id, list], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
    })?;
    for row in rows {
        if let (format, Some(deck_id)) = row? {
            found.insert(format, deck_id);
        }
    }
    Ok(found)
}

/// The deck of the most recently started MTGO match that has a deck version,
/// with its cover (0.x `FindLastPlayedDeck`): the idle card's deck and the
/// settings preview's real deck.
pub fn last_played_deck(conn: &Connection) -> Result<Option<OverlayDeck>, StoreError> {
    Ok(conn
        .prepare_cached(&last_played_deck_sql())?
        .query_row([], |row| deck_row(row, 0))
        .optional()?)
}

/// `last_played_deck`'s query. The CROSS JOIN keeps the matches outside and
/// the unary `+` keeps the account and deck indexes out, so SQLite walks
/// `matches_started_at` newest first and stops at the first match with a
/// deck, instead of sorting every match that has one (9 ms at 30,000
/// matches).
fn last_played_deck_sql() -> String {
    format!(
        "SELECT {DECK_COLUMNS}
         FROM listed_matches m CROSS JOIN deck_versions v
             JOIN decks d ON d.id = v.deck_id
             LEFT JOIN cards c ON c.catalog_id = d.cover_catalog_id
         WHERE v.id = m.deck_version_id AND +m.deck_version_id IS NOT NULL
           AND {mtgo}
         ORDER BY m.started_at DESC, m.id DESC LIMIT 1",
        mtgo = account_of_mtgo("+m.account_id"),
    )
}

/// The account of the newest `login` event and its stored login id (0.x
/// `FindLocalLoginId`, spec 10): the username the line names, and that
/// account's `accounts.login_id`, never guessed from the line. Within a
/// file the last line by position is its newest (log times can slip,
/// #303 F6); across files the newest of those by time wins, since two
/// clients write two files.
pub fn current_login(conn: &Connection) -> Result<Option<(String, Option<i64>)>, StoreError> {
    let row = conn
        .prepare_cached(
            "SELECT e.raw, e.compressed FROM log_events e
             WHERE e.kind = 'login' AND e.quarantined = 0
               AND e.byte_offset = (
                   SELECT MAX(l.byte_offset) FROM log_events l
                   WHERE l.kind = 'login' AND l.quarantined = 0
                     AND l.log_file_id = e.log_file_id)
             ORDER BY e.logged_at DESC, e.log_file_id DESC LIMIT 1",
        )?
        .query_row([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
        })
        .optional()?;
    let Some((raw, compressed)) = row else {
        return Ok(None);
    };
    let Some(login) = mtgo_protocol::parse_login(&decode_raw(raw, compressed)?) else {
        return Ok(None);
    };
    let login_id: Option<i64> = conn
        .prepare_cached("SELECT login_id FROM accounts WHERE username = ?1")?
        .query_row([&login.username], |row| row.get(0))
        .optional()?
        .flatten();
    Ok(Some((login.username, login_id)))
}

/// A deck version's cards, main and sideboard, from its signature (the
/// match's deck version when the current game has no `deck_used` list,
/// spec 7.4). `None` for an unknown version.
pub fn deck_version_cards(
    conn: &Connection,
    version_id: i64,
) -> Result<Option<Vec<DeckCard>>, StoreError> {
    let signature: Option<String> = conn
        .prepare_cached("SELECT signature FROM deck_versions WHERE id = ?1")?
        .query_row([version_id], |row| row.get(0))
        .optional()?;
    Ok(signature.map(|signature| decks::decode_signature(&signature)))
}

/// The `cards` rows of `catalog_ids` (one query; unknown ids are left out).
/// The overlay asks for every printing a frame, a reveal or the deck names,
/// so a back face or another printing matches by oracle id or face name
/// (U1, `CardMeta`).
pub fn card_meta(conn: &Connection, catalog_ids: &[u32]) -> Result<CardMeta, StoreError> {
    let mut meta = CardMeta::default();
    if catalog_ids.is_empty() {
        return Ok(meta);
    }
    let ids: Vec<String> = catalog_ids.iter().map(u32::to_string).collect();
    let list = format!("[{}]", ids.join(","));
    let mut stmt = conn.prepare_cached(
        "SELECT catalog_id, name, oracle_id, type, color_identity, image, art_crop,
             local_image, local_art_crop, mana_cost
         FROM cards WHERE catalog_id IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([list], |row| {
        Ok((
            row.get::<_, u32>(0)?,
            CardInfo {
                name: row.get(1)?,
                oracle_id: row.get(2)?,
                card_type: row.get(3)?,
                color_identity: row.get(4)?,
                image: row.get(5)?,
                art_crop: row.get(6)?,
                local_image: row.get(7)?,
                local_art_crop: row.get(8)?,
                mana_cost: row.get(9)?,
            },
        ))
    })?;
    for row in rows {
        let (catalog_id, info) = row?;
        meta.cards.insert(catalog_id, info);
    }
    Ok(meta)
}

/// The archetype on the opponent seat of the newest earlier match against
/// `opponent_id` in this format (0.x `ResolveOverlayOpponent::lastFaced`,
/// spec 7.3), hand-set or detected; the live match is left out.
pub fn last_faced_archetype(
    conn: &Connection,
    opponent_id: i64,
    format: Option<&str>,
    exclude_match_id: i64,
) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT a.archetype_id
             FROM listed_matches m
                 JOIN match_archetypes a ON a.match_id = m.id AND a.is_opponent = 1
             WHERE m.opponent_id = ?1 AND m.format IS ?2 AND m.id <> ?3
             ORDER BY m.started_at DESC, m.id DESC LIMIT 1",
        )?
        .query_row(params![opponent_id, format, exclude_match_id], |row| {
            row.get(0)
        })
        .optional()?)
}

/// A cheap check of a format's held candidates (`archetypes::held_candidates`):
/// the number of the format's archetype rows and their newest `updated_at`
/// (spec 14, the game view budget). It is a second guard only: not every
/// change moves it (removing a variant does not, nor do `cards` rows the
/// lists join), so the game overlay also forgets its candidates on the
/// tracker's `ArchetypesChanged` (every archetype edit) and `CardsChanged`.
/// One small read.
pub fn candidates_stamp(conn: &Connection, format: &str) -> Result<(i64, i64), StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT COUNT(*), COALESCE(MAX(updated_at), 0) FROM archetypes WHERE format = ?1",
        )?
        .query_row([format], |row| Ok((row.get(0)?, row.get(1)?)))?)
}

/// The hand-set opponent archetype of a match (0.x's `manual` row).
pub fn manual_opponent_archetype(
    conn: &Connection,
    match_id: i64,
) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT archetype_id FROM match_archetypes
             WHERE match_id = ?1 AND is_opponent = 1 AND manual = 1",
        )?
        .query_row([match_id], |row| row.get(0))
        .optional()?)
}

/// The planned cards of the deck's guide against the archetype, in
/// `(direction, oracle_id)` order; empty without a guide.
pub fn guide_plan(
    conn: &Connection,
    deck_id: i64,
    archetype_id: i64,
) -> Result<Vec<archetypes::guides::GuideCard>, StoreError> {
    let guide: Option<i64> = conn
        .prepare_cached("SELECT id FROM sideboard_guides WHERE deck_id = ?1 AND archetype_id = ?2")?
        .query_row(params![deck_id, archetype_id], |row| row.get(0))
        .optional()?;
    match guide {
        Some(guide_id) => crate::guides::guide_cards(conn, guide_id),
        None => Ok(Vec::new()),
    }
}

/// One card row per oracle id, the newest printing (highest catalog id).
pub fn cards_by_oracle(
    conn: &Connection,
    oracle_ids: &[String],
) -> Result<std::collections::BTreeMap<String, CardInfo>, StoreError> {
    let mut out = std::collections::BTreeMap::new();
    if oracle_ids.is_empty() {
        return Ok(out);
    }
    let list = serde_json::to_string(oracle_ids)
        .map_err(|error| StoreError::Corrupt(format!("oracle ids: {error}")))?;
    let mut stmt = conn.prepare_cached(
        "SELECT oracle_id, name, type, color_identity, image, art_crop, local_image,
             local_art_crop
         FROM cards WHERE oracle_id IN (SELECT value FROM json_each(?1))
         ORDER BY catalog_id DESC",
    )?;
    let rows = stmt.query_map([list], |row| {
        Ok((
            row.get::<_, String>(0)?,
            CardInfo {
                name: row.get(1)?,
                oracle_id: row.get(0)?,
                card_type: row.get(2)?,
                color_identity: row.get(3)?,
                image: row.get(4)?,
                art_crop: row.get(5)?,
                local_image: row.get(6)?,
                local_art_crop: row.get(7)?,
                mana_cost: None,
            },
        ))
    })?;
    for row in rows {
        let (oracle_id, info) = row?;
        // Newest first: the first row of an oracle id is kept.
        out.entry(oracle_id).or_insert(info);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use mtgo_match::{GameResult, MatchState, MatchView, Outcome};
    use mtgo_protocol::{EventKind, Subjects};
    use overlay::{GRACE_MS, finished_ms};

    use super::*;
    use crate::test_support::{
        db, event_with, game, hide, match_id, new_log_file, number, store_events, view,
    };
    use crate::write::{MatchWrite, NewEvent, WriteBatch, commit, insert_log_file};

    fn store_views(conn: &mut Connection, views: Vec<MatchView>) {
        let matches = views
            .into_iter()
            .map(|view| MatchWrite::Upsert {
                view: Box::new(view),
                revision: 1,
            })
            .collect();
        commit(
            conn,
            &WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        )
        .expect("matches");
    }

    fn file(conn: &Connection, path: &str) -> i64 {
        insert_log_file(conn, &new_log_file(path)).expect("file")
    }

    /// A state frame of `game_id` whose text names `marker` (so a test can
    /// tell frames apart), padded with `cards` cards.
    fn frame_text(game_id: u64, marker: &str, cards: usize) -> String {
        let cards: Vec<String> = (0..cards)
            .map(|index| {
                format!(
                    "{{\"Id\":{index},\"CatalogID\":{},\"Zone\":\"Battlefield\",\"ActualZone\":\"Battlefield\",\"Owner\":1,\"Controller\":1}}",
                    1000 + index
                )
            })
            .collect();
        format!(
            "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: 99, Event ID: 99) {{\"Marker\":\"{marker}\",\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":40,\"HandCount\":7,\"Life\":20}}],\"Cards\":[{}]}}",
            cards.join(",")
        )
    }

    fn frame(file: i64, offset: u64, game_id: u64, marker: &str) -> NewEvent {
        event_with(
            file,
            offset,
            EventKind::GameStateUpdate,
            Subjects {
                match_id: Some(99),
                game_id: Some(game_id),
                ..Subjects::default()
            },
            &frame_text(game_id, marker, 3),
        )
    }

    /// A state frame of `game_id` with both seats, the local one on `life`.
    fn seated_frame(file: i64, offset: u64, game_id: u64, life: u32) -> NewEvent {
        event_with(
            file,
            offset,
            EventKind::GameStateUpdate,
            Subjects {
                match_id: Some(99),
                game_id: Some(game_id),
                ..Subjects::default()
            },
            &format!(
                "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: 99, Event ID: 99) {{\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":40,\"HandCount\":7,\"Life\":{life}}},{{\"Id\":2,\"Name\":\"Qrival\",\"LibraryCount\":40,\"HandCount\":7,\"Life\":20}}],\"Cards\":[]}}"
            ),
        )
    }

    fn deck_used(file: i64, offset: u64, game_id: u64, cards: &str) -> NewEvent {
        event_with(
            file,
            offset,
            EventKind::DeckUsed,
            Subjects {
                game_id: Some(game_id),
                ..Subjects::default()
            },
            &format!(
                "12:00:00 [INF] (Twitch Info|Username: Qlocal Deck Used in Game ID: {game_id}) {cards}"
            ),
        )
    }

    fn state_change(file: i64, offset: u64, token: &str, from: &str, to: &str) -> NewEvent {
        event_with(
            file,
            offset,
            EventKind::MatchStateChanged,
            Subjects {
                match_token: Some(token.to_string()),
                ..Subjects::default()
            },
            &format!(
                "12:00:00 [INF] (Game Management|Match State Changed for {token} from {from} to {to})"
            ),
        )
    }

    const LIVE: &str = "11111111-1111-4111-8111-111111111111";

    /// A live match against Qrival: game 1 (Game ID 701) won, game 2 (702)
    /// under way.
    fn live_view() -> MatchView {
        let mut live = view(LIVE, "Qrival", 50_000);
        live.state = MatchState::InProgress;
        live.outcome = Outcome::Unknown;
        live.ended_at_ms = None;
        live.games = vec![
            game(1, Some(701), GameResult::Win),
            game(2, Some(702), GameResult::Unknown),
        ];
        live.wins = 1;
        live.losses = 0;
        live
    }

    // Frames

    #[test]
    fn latest_frame_uses_the_frames_index() {
        let (_dir, conn) = db("overlay-frames-index");
        let plan: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {LATEST_FRAME}"))
                .expect("explain");
            stmt.query_map([7], |row| row.get::<_, String>(3))
                .expect("plan")
                .map(|row| row.expect("row"))
                .collect()
        };
        let plan = plan.join(" | ");
        assert!(plan.contains("USING INDEX log_events_frames"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "no sort: {plan}");
        let plan: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {FIRST_FRAME}"))
                .expect("explain");
            stmt.query_map([7], |row| row.get::<_, String>(3))
                .expect("plan")
                .map(|row| row.expect("row"))
                .collect()
        };
        let plan = plan.join(" | ");
        assert!(plan.contains("log_events_frames"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "no sort: {plan}");
        let plan: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {FIRST_FRAME_RAW}"))
                .expect("explain");
            stmt.query_map([7], |row| row.get::<_, String>(3))
                .expect("plan")
                .map(|row| row.expect("row"))
                .collect()
        };
        let plan = plan.join(" | ");
        assert!(plan.contains("log_events_frames"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "no sort: {plan}");
    }

    #[test]
    fn the_newest_frame_is_the_last_in_log_order_and_is_decoded() {
        let (_dir, mut conn) = db("overlay-newest-frame");
        let first = file(&conn, "Q/first.log");
        let second = file(&conn, "Q/second.log");
        // A big frame is stored compressed; log order is (file, offset),
        // never the time: the second file's frame is newest although it is
        // dated earlier.
        let mut newest = event_with(
            second,
            100,
            EventKind::GameStateUpdate,
            Subjects {
                match_id: Some(99),
                game_id: Some(7),
                ..Subjects::default()
            },
            &frame_text(7, "newest", 400),
        );
        newest.logged_at_ms = 1;
        store_events(
            &mut conn,
            vec![
                frame(first, 900, 7, "older"),
                frame(first, 100, 7, "oldest"),
                newest,
                frame(second, 50, 8, "another game"),
            ],
        );
        assert_eq!(
            number(
                &conn,
                "SELECT compressed FROM log_events WHERE byte_offset = 100 AND log_file_id = 2"
            ),
            1,
            "the newest frame is stored compressed"
        );

        let text = latest_frame(&conn, 7).expect("read").expect("a frame");
        assert!(text.contains("\"Marker\":\"newest\""), "{text}");
        assert_eq!(first_frame_at(&conn, 7).expect("read"), Some((first, 100)));
        assert_eq!(latest_frame(&conn, 9).expect("read"), None);
        assert_eq!(first_frame_at(&conn, 9).expect("read"), None);
    }

    #[test]
    #[ignore = "speed measurement, run in release: cargo test -p store --release -- --ignored"]
    fn newest_frame_read_under_1_ms() {
        let (_dir, mut conn) = db("overlay-frame-speed");
        let log = file(&conn, "Q/mtgo.log");
        // 2,000 frames of one game growing to 75 cards, beside 2,000 of
        // another game.
        let mut events = Vec::new();
        for index in 0..2_000u64 {
            let cards = (index as usize * 75) / 2_000;
            for (game_id, base) in [(7u64, 0u64), (8, 1)] {
                events.push(event_with(
                    log,
                    index * 10 + base,
                    EventKind::GameStateUpdate,
                    Subjects {
                        match_id: Some(99),
                        game_id: Some(game_id),
                        ..Subjects::default()
                    },
                    &frame_text(game_id, &format!("{index}"), cards),
                ));
            }
        }
        store_events(&mut conn, events);
        let mut times = Vec::new();
        for _ in 0..51 {
            let started = Instant::now();
            let text = latest_frame(&conn, 7).expect("read").expect("frame");
            times.push(started.elapsed());
            assert!(text.contains("\"Marker\":\"1999\""));
        }
        times.sort();
        let median = times[times.len() / 2];
        assert!(
            median.as_micros() < 1_000,
            "the newest frame read took {median:?}"
        );
    }

    // The live match

    #[test]
    fn live_game_inputs_pick_each_games_first_deck_list_and_newest_frame() {
        let (_dir, mut conn) = db("overlay-live-inputs");
        let log = file(&conn, "Q/mtgo.log");
        store_views(&mut conn, vec![live_view()]);
        store_events(
            &mut conn,
            vec![
                deck_used(
                    log,
                    10,
                    701,
                    r#"[{"CatalogId":1001,"Quantity":4,"InSideboard":false},{"CatalogId":2001,"Quantity":2,"InSideboard":true}]"#,
                ),
                frame(log, 20, 701, "g1 first"),
                frame(log, 30, 701, "g1 last"),
                // Sideboarded: game 2's list differs, and a second list for
                // the same game is ignored (0.x takes the first).
                deck_used(
                    log,
                    40,
                    702,
                    r#"[{"CatalogId":1001,"Quantity":3,"InSideboard":false},{"CatalogId":2001,"Quantity":1,"InSideboard":false}]"#,
                ),
                deck_used(
                    log,
                    45,
                    702,
                    r#"[{"CatalogId":9999,"Quantity":60,"InSideboard":false}]"#,
                ),
                frame(log, 50, 702, "g2 first"),
                frame(log, 60, 702, "g2 last"),
            ],
        );

        let inputs = live_game_inputs(&conn, LIVE).expect("read").expect("live");
        assert_eq!(inputs.match_id, match_id(&conn, LIVE));
        assert_eq!(inputs.local_username, "Qlocal");
        assert_eq!(inputs.format.as_deref(), Some("CMODERN"));
        assert!(inputs.opponent_id.is_some());
        assert_eq!(inputs.deck_version_id, None);
        assert_eq!(inputs.games.len(), 2);

        let first = &inputs.games[0];
        assert_eq!((first.number, first.mtgo_game_id), (1, Some(701)));
        assert_eq!(
            first.deck_used,
            Some(vec![
                DeckCard {
                    catalog_id: 1001,
                    quantity: 4,
                    sideboard: false
                },
                DeckCard {
                    catalog_id: 2001,
                    quantity: 2,
                    sideboard: true
                },
            ])
        );
        assert!(
            first
                .newest_frame
                .as_deref()
                .expect("frame")
                .contains("g1 last")
        );
        assert_eq!(first.first_frame_at, Some((log, 20)));

        let second = &inputs.games[1];
        assert_eq!((second.number, second.mtgo_game_id), (2, Some(702)));
        assert_eq!(
            second.deck_used,
            Some(vec![
                DeckCard {
                    catalog_id: 1001,
                    quantity: 3,
                    sideboard: false
                },
                DeckCard {
                    catalog_id: 2001,
                    quantity: 1,
                    sideboard: false
                },
            ]),
            "the game's own first list, not game 1's or a later one"
        );
        assert!(
            second
                .newest_frame
                .as_deref()
                .expect("frame")
                .contains("g2 last")
        );
        assert_eq!(second.first_frame_at, Some((log, 50)));

        assert_eq!(
            live_game_inputs(&conn, "no-such-token").expect("read"),
            None
        );
    }

    #[test]
    fn live_game_inputs_carry_each_games_play_draw_and_mulligans() {
        let (_dir, mut conn) = db("overlay-live-play-draw");
        let mut live = live_view();
        live.games[0].on_play = Some(false);
        live.games[0].local_mulligans = Some(1);
        live.games[0].opp_mulligans = Some(0);
        // Game 2 has started, but nothing is chosen or kept yet.
        live.games[1].on_play = None;
        live.games[1].local_mulligans = None;
        live.games[1].opp_mulligans = None;
        store_views(&mut conn, vec![live]);

        let inputs = live_game_inputs(&conn, LIVE).expect("read").expect("live");

        let first = &inputs.games[0];
        assert_eq!(first.on_play, Some(false));
        assert_eq!(
            (first.local_mulligans, first.opp_mulligans),
            (Some(1), Some(0))
        );
        let second = &inputs.games[1];
        assert_eq!(second.on_play, None);
        assert_eq!((second.local_mulligans, second.opp_mulligans), (None, None));
    }

    #[test]
    fn a_game_without_events_has_no_deck_list_or_frame() {
        let (_dir, mut conn) = db("overlay-live-empty");
        store_views(&mut conn, vec![live_view()]);
        let inputs = live_game_inputs(&conn, LIVE).expect("read").expect("live");
        for game in &inputs.games {
            assert_eq!(game.deck_used, None);
            assert_eq!(game.newest_frame, None);
            assert_eq!(game.first_frame_at, None);
        }
    }

    #[test]
    fn sideboarding_events_are_the_matchs_sideboarding_states_in_log_order() {
        let (_dir, mut conn) = db("overlay-sideboarding-events");
        let first = file(&conn, "Q/first.log");
        let second = file(&conn, "Q/second.log");
        let long_name = format!("Q{}SideboardingState", "x".repeat(600));
        store_events(
            &mut conn,
            vec![
                state_change(
                    second,
                    10,
                    LIVE,
                    "GameOverState",
                    "LeagueMatchSideboardingState",
                ),
                state_change(first, 90, LIVE, "A", "MatchJoinedEventUnderwayState"),
                state_change(first, 50, LIVE, "GameOverState", "MatchSideboardingState"),
                // A long line is stored compressed and still found.
                state_change(first, 70, LIVE, "Q", &long_name),
                state_change(
                    first,
                    60,
                    "another-token",
                    "GameOverState",
                    "MatchSideboardingState",
                ),
            ],
        );
        let events = sideboarding_events(&conn, LIVE).expect("read");
        let positions: Vec<Position> = events.iter().map(|(at, _)| *at).collect();
        assert_eq!(positions, vec![(first, 50), (first, 70), (second, 10)]);
        assert!(events[0].1.contains("to MatchSideboardingState"));
        assert!(events[1].1.contains(&long_name));
        assert!(events[2].1.contains("to LeagueMatchSideboardingState"));
    }

    // Head to head

    fn past(token: &str, opponent: &str, format: &str, outcome: Outcome, at: i64) -> MatchView {
        let mut past = view(token, opponent, at);
        past.format = Some(format.to_string());
        past.outcome = outcome;
        past
    }

    fn opponent_id(conn: &Connection, name: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM opponents WHERE username = ?1",
            [name],
            |row| row.get(0),
        )
        .expect("opponent")
    }

    /// 0.x "counts the head-to-head record in this format only".
    #[test]
    fn head_to_head_counts_this_format_only() {
        let (_dir, mut conn) = db("overlay-head-to-head");
        store_views(
            &mut conn,
            vec![
                past("q-h2h-modern", "Qrival", "CMODERN", Outcome::Win, 1_000),
                past("q-h2h-pauper-1", "Qrival", "CPAUPER", Outcome::Loss, 2_000),
                past("q-h2h-pauper-2", "Qrival", "CPAUPER", Outcome::Loss, 3_000),
                live_view(),
            ],
        );
        let rival = opponent_id(&conn, "Qrival");
        let live = match_id(&conn, LIVE);
        assert_eq!(
            head_to_head(&conn, rival, Some("CMODERN"), live).expect("read"),
            (1, 0)
        );
        assert_eq!(
            head_to_head(&conn, rival, Some("CPAUPER"), live).expect("read"),
            (0, 2)
        );
    }

    #[test]
    fn head_to_head_counts_complete_listed_matches_but_the_live_one() {
        let (_dir, mut conn) = db("overlay-head-to-head-rules");
        let mut ended = past("q-ended", "Qrival", "CMODERN", Outcome::Win, 4_000);
        ended.state = MatchState::Ended;
        let mut live_complete = live_view();
        live_complete.state = MatchState::Complete;
        live_complete.outcome = Outcome::Win;
        store_views(
            &mut conn,
            vec![
                past("q-win", "Qrival", "CMODERN", Outcome::Win, 1_000),
                past("q-draw", "Qrival", "CMODERN", Outcome::Draw, 2_000),
                past("q-hidden", "Qrival", "CMODERN", Outcome::Loss, 3_000),
                past("q-other", "Qsomeone", "CMODERN", Outcome::Loss, 3_500),
                ended,
                live_complete,
            ],
        );
        hide(&conn, "q-hidden");
        let rival = opponent_id(&conn, "Qrival");
        let live = match_id(&conn, LIVE);
        // The draw counts in neither; the hidden, the ended and the live
        // match are not history; another opponent's loss is theirs.
        assert_eq!(
            head_to_head(&conn, rival, Some("CMODERN"), live).expect("read"),
            (1, 0)
        );
        // A match without a format meets only matches without one.
        assert_eq!(
            head_to_head(&conn, rival, None, live).expect("read"),
            (0, 0)
        );
    }

    // The league card

    const NOW: i64 = 10_000_000;

    fn insert_deck(conn: &Connection, id: i64, name: &str, cover: Option<u32>) {
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, name, format, color_identity, cover_catalog_id,
                 created_at, updated_at, source)
             VALUES (?1, ?2, ?3, 'CMODERN', 'G,R,W', ?4, 1, 1, 'file')",
            params![id, format!("q-deck-{id}"), name, cover],
        )
        .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (?1, ?1, 'SIG', 1)",
            [id],
        )
        .expect("version");
    }

    fn insert_card(conn: &Connection, catalog_id: u32, name: &str) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity, image, art_crop,
                 local_image, local_art_crop)
             VALUES (?1, ?2, ?3, 'Creature', 'G', ?4, ?5, ?6, ?7)",
            params![
                catalog_id,
                format!("oracle-{name}"),
                name,
                format!("https://img/{catalog_id}.jpg"),
                format!("https://art/{catalog_id}.jpg"),
                format!("{catalog_id}.jpg"),
                format!("{catalog_id}_art.jpg"),
            ],
        )
        .expect("card");
    }

    struct Run<'a> {
        id: i64,
        state: &'a str,
        started_at: i64,
        completed_at: Option<i64>,
        dropped_at: Option<i64>,
        deck_id: Option<i64>,
        deck_version_id: Option<i64>,
    }

    fn insert_run(conn: &Connection, account: i64, run: &Run) {
        conn.execute(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name, format,
                 deck_version_id, deck_id, started_at, completed_at, dropped_at, created_at)
             VALUES (?1, 'run-' || ?1, ?2, 'log', 'constructed', ?3, 'q-league-' || ?1,
                 'Modern League 14-09-2026 11:01am', 'CMODERN', ?4, ?5, ?6, ?7, ?8, 1)",
            params![
                run.id,
                account,
                run.state,
                run.deck_version_id,
                run.deck_id,
                run.started_at,
                run.completed_at,
                run.dropped_at,
            ],
        )
        .expect("run");
    }

    /// A match against Qrival with three games (won, lost, won).
    fn member(token: &str, at: i64, state: MatchState, outcome: Outcome) -> MatchView {
        let mut member = view(token, "Qrival", at);
        member.state = state;
        member.outcome = outcome;
        member
    }

    fn join_run(conn: &Connection, run: i64, token: &str) {
        conn.execute(
            "UPDATE matches SET league_id = ?1 WHERE token = ?2",
            params![run, token],
        )
        .expect("join");
    }

    fn account_id(conn: &Connection) -> i64 {
        number(conn, "SELECT id FROM accounts WHERE username = 'Qlocal'")
    }

    fn run_ids(input: &LeagueCardInput) -> Vec<i64> {
        input.runs.iter().map(|run| run.id).collect()
    }

    /// A stored standing for `event`, waiting for its next round.
    fn store_tournament(conn: &Connection, account: i64, event: i64, round: Option<i64>) {
        crate::tournament_sync::write_standing(
            conn,
            &crate::tournament_sync::StandingRow {
                account_id: account,
                mtgo_event_id: event,
                token: format!("q-token-{event}"),
                name: Some("Modern Challenge 64".to_string()),
                format: Some("CMODERN".to_string()),
                cut_size: Some(8),
                swiss_rounds: Some(6),
                round_minutes: Some(50),
                starts_at: Some(NOW - HOUR_MS),
                status: if round.is_some() {
                    leagues::standing::StandingStatus::WaitingForRound
                } else {
                    leagues::standing::StandingStatus::Registered
                },
                round,
                stage: leagues::standing::StandingStage::Swiss,
                in_playoffs: false,
                round_started_at: None,
                round_result_at: None,
                rank: None,
                players: None,
                matches_left: Some(12),
                byes: 0,
                finish: None,
                place: None,
                finished_at: None,
                last_transition_at: NOW - 2 * 60_000,
                last_event_at: NOW - 60_000,
            },
        )
        .expect("standing");
    }

    const HOUR_MS: i64 = 60 * 60_000;

    #[test]
    fn league_card_inputs_read_the_tournaments_their_matches_and_decks() {
        let (_dir, mut conn) = db("overlay-league-tournaments");
        insert_deck(&conn, 1, "Qformat deck", None);
        insert_deck(&conn, 2, "Qround deck", None);
        let mut older = member("q-older", 50, MatchState::Complete, Outcome::Win);
        older.format = Some("CMODERN".to_string());
        let mut round_two = member("q-t700-r2", 200, MatchState::InProgress, Outcome::Unknown);
        round_two.tournament_event_id = Some(700);
        round_two.tournament_round = Some(2);
        store_views(&mut conn, vec![older, round_two]);
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 1 WHERE token = 'q-older';
             UPDATE matches SET deck_version_id = 2 WHERE token = 'q-t700-r2';",
        )
        .expect("decks");
        let account = account_id(&conn);
        store_tournament(&conn, account, 700, Some(2));
        store_tournament(&conn, account, 800, None);
        // No deck of its own: its format's last played deck.
        store_tournament(&conn, account, 850, None);
        // Registered long before its start: read while it waits.
        store_tournament(&conn, account, 870, None);
        conn.execute(
            "UPDATE tournament_standings SET last_event_at = ?1, starts_at = ?2
             WHERE mtgo_event_id = 870",
            [NOW - overlay::STALE_AFTER_MS - 1, NOW + HOUR_MS],
        )
        .expect("early");
        // A tournament with no fact for 90 minutes is not read.
        store_tournament(&conn, account, 900, Some(4));
        conn.execute(
            "UPDATE tournament_standings SET last_event_at = ?1 WHERE mtgo_event_id = 900",
            [NOW - overlay::STALE_AFTER_MS - 1],
        )
        .expect("stale");

        // Tournament 800's registered list is deck 3's version.
        insert_deck(&conn, 3, "Qregistered deck", None);
        conn.execute("UPDATE decks SET account_id = ?1", [account])
            .expect("the account's decks");
        let line = "(Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in X) \
             Message: {\"TournamentID\":800,\"DeckCards\":[{\"CatalogID\":1001,\"Quantity\":4,\
             \"InSideboard\":false}],\"ReturnCode\":0}";
        let (_, cards) = decks::parse_tournament_deck(line).expect("a tournament deck line");
        conn.execute(
            "UPDATE deck_versions SET signature = ?1 WHERE id = 3",
            [decks::signature(&cards)],
        )
        .expect("signature");
        let log = file(&conn, "Q/mtgo.log");
        store_events(
            &mut conn,
            vec![event_with(
                log,
                10,
                EventKind::MatchDeckRegistered,
                Subjects::default(),
                line,
            )],
        );

        let mut queries = 0;
        let input =
            league_card_inputs_counted(&conn, account, NOW, GRACE_MS, &mut queries).expect("read");
        // The eleven, plus two for the registered deck and one for the
        // format deck.
        assert_eq!(queries, 11, "{queries} queries");
        let events: Vec<i64> = input
            .tournaments
            .iter()
            .map(|tournament| tournament.mtgo_event_id)
            .collect();
        assert_eq!(events, vec![700, 800, 850, 870]);
        let paired = &input.tournaments[0];
        assert_eq!(paired.matches.len(), 1);
        assert_eq!(paired.matches[0].round, Some(2));
        assert_eq!(
            paired.matches[0].deck.as_ref().map(|deck| deck.id),
            Some(2),
            "the round's match deck"
        );
        assert_eq!(
            paired.format_deck, None,
            "a tournament with a match deck reads no format deck"
        );
        let registered = &input.tournaments[1];
        assert!(registered.matches.is_empty());
        assert_eq!(
            registered.registered_deck.as_ref().map(|deck| deck.id),
            Some(3),
            "the registered list's deck"
        );
        assert_eq!(registered.format_deck, None);
        assert_eq!(
            input.tournaments[2]
                .format_deck
                .as_ref()
                .map(|deck| deck.id),
            Some(2),
            "the format's last played deck is the newest match's"
        );

        let card = overlay::league_state(input, NOW, GRACE_MS, &overlay::CardLook::default());
        assert_eq!(card.status, overlay::LeagueStatus::InGame);
        assert_eq!(card.tournament.map(|block| block.round), Some(2));
    }

    fn plan(conn: &Connection, sql: &str, params: impl rusqlite::Params) -> String {
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map(params, |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        details.join("\n")
    }

    #[test]
    fn the_tournament_deck_reads_walk_their_indexes() {
        let (_dir, conn) = db("overlay-tournament-deck-plans");
        let format = plan(&conn, FORMAT_DECKS, params![1, "[\"CMODERN\"]"]);
        assert!(
            format.contains("matches_account_format_started"),
            "plan:\n{format}"
        );
        assert!(!format.contains("TEMP B-TREE"), "no sort:\n{format}");

        let lines = plan(&conn, RECENT_TOURNAMENT_DECK_RAWS, [0]);
        assert!(lines.contains("log_events_registered"), "plan:\n{lines}");
        assert!(!lines.contains("SCAN log_events"), "plan:\n{lines}");
        let last = plan(&conn, &last_played_deck_sql(), []);
        assert!(last.contains("matches_started_at"), "plan:\n{last}");
        assert!(!last.contains("TEMP B-TREE"), "no sort:\n{last}");
        let versions = plan(
            &conn,
            ACCOUNT_VERSIONS_BY_SIGNATURE,
            params![1, "[\"SIG\"]"],
        );
        assert!(versions.contains("decks_account_id"), "plan:\n{versions}");
    }

    /// Run 1 active (4 results and a live match with two games, one won),
    /// its deck by its own `deck_id`.
    fn active_run(conn: &mut Connection) -> i64 {
        insert_card(conn, 5001, "Qcover");
        insert_deck(conn, 1, "Qdeck one", Some(5001));
        let mut live = member("q-r1-live", 6_000, MatchState::InProgress, Outcome::Unknown);
        live.games = vec![
            game(1, Some(801), GameResult::Win),
            game(2, Some(802), GameResult::Unknown),
        ];
        store_views(
            conn,
            vec![
                member("q-r1-1", 1_000, MatchState::Complete, Outcome::Win),
                member("q-r1-2", 2_000, MatchState::Complete, Outcome::Win),
                member("q-r1-3", 3_000, MatchState::Complete, Outcome::Loss),
                member("q-r1-4", 4_000, MatchState::Abandoned, Outcome::Unknown),
                live,
            ],
        );
        let account = account_id(conn);
        insert_run(
            conn,
            account,
            &Run {
                id: 1,
                state: "active",
                started_at: 500,
                completed_at: None,
                dropped_at: None,
                deck_id: Some(1),
                deck_version_id: None,
            },
        );
        for token in ["q-r1-1", "q-r1-2", "q-r1-3", "q-r1-4", "q-r1-live"] {
            join_run(conn, 1, token);
        }
        account
    }

    #[test]
    fn league_card_inputs_read_the_live_games_frames() {
        let (_dir, mut conn) = db("overlay-league-frames");
        let account = active_run(&mut conn);
        let log = file(&conn, "Q/mtgo.log");
        store_events(
            &mut conn,
            vec![
                frame(log, 10, 801, "g1"),
                frame(log, 30, 802, "g2 first"),
                frame(log, 40, 802, "g2 newest"),
            ],
        );
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(input.local_username.as_deref(), Some("Qlocal"));
        for member in input.runs.iter().flat_map(|run| &run.matches) {
            if member.token == "q-r1-live" {
                assert!(
                    member
                        .first_frame
                        .as_deref()
                        .expect("first")
                        .contains("g2 first")
                );
                assert!(
                    member
                        .newest_frame
                        .as_deref()
                        .expect("newest")
                        .contains("g2 newest")
                );
            } else {
                assert_eq!((&member.first_frame, &member.newest_frame), (&None, &None));
            }
        }
    }

    #[test]
    fn league_card_inputs_read_the_frames_of_the_match_the_card_shows() {
        // Two active runs each with a live match: run 1's live match started
        // last (the newest real transition, event overlay spec 6), so the
        // card shows run 1, its match carries the frames and the card has
        // life.
        let (_dir, mut conn) = db("overlay-league-two-live");
        let account = active_run(&mut conn);
        let mut other = member("q-r2-live", 5_500, MatchState::InProgress, Outcome::Unknown);
        other.games = vec![game(1, Some(901), GameResult::Unknown)];
        store_views(&mut conn, vec![other]);
        insert_run(
            &conn,
            account,
            &Run {
                id: 2,
                state: "active",
                started_at: 100,
                completed_at: None,
                dropped_at: None,
                deck_id: None,
                deck_version_id: None,
            },
        );
        join_run(&conn, 2, "q-r2-live");
        let log = file(&conn, "Q/mtgo.log");
        store_events(
            &mut conn,
            vec![
                seated_frame(log, 30, 802, 20),
                seated_frame(log, 40, 802, 14),
                seated_frame(log, 50, 901, 20),
                seated_frame(log, 60, 901, 7),
            ],
        );
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        let framed: Vec<&str> = input
            .runs
            .iter()
            .flat_map(|run| &run.matches)
            .filter(|member| member.newest_frame.is_some())
            .map(|member| member.token.as_str())
            .collect();
        assert_eq!(framed, vec!["q-r1-live"]);
        let card = overlay::league_state(input, NOW, GRACE_MS, &overlay::CardLook::default());
        let live = card.live_match.expect("live");
        assert_eq!(live.number, 4, "run 1's live match");
        assert_eq!(
            live.life.map(|life| life.you),
            Some(14),
            "the life of the shown match"
        );
    }

    #[test]
    fn league_card_inputs_read_the_run_its_matches_games_and_deck() {
        let (_dir, mut conn) = db("overlay-league-inputs");
        let account = active_run(&mut conn);
        let log = file(&conn, "Q/mtgo.log");
        store_events(
            &mut conn,
            vec![
                frame(log, 10, 801, "g1"),
                state_change(
                    log,
                    20,
                    "q-r1-live",
                    "GameOverState",
                    "LeagueMatchSideboardingState",
                ),
                frame(log, 5, 802, "g2 quarantined"),
                frame(log, 30, 802, "g2 first"),
                frame(log, 40, 802, "g2 later"),
            ],
        );
        conn.execute(
            "UPDATE log_events SET quarantined = 1 WHERE byte_offset = 5",
            [],
        )
        .expect("quarantine");
        conn.execute("UPDATE decks SET overlay_label = 'Qlabel' WHERE id = 1", [])
            .expect("label");

        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), vec![1]);
        let run = &input.runs[0];
        assert_eq!(run.kind, leagues::LeagueKind::Constructed);
        assert_eq!(run.state, leagues::RunState::Active);
        assert_eq!(run.name, "Modern League 14-09-2026 11:01am");
        assert_eq!(run.format.as_deref(), Some("CMODERN"));
        let tokens: Vec<&str> = run.matches.iter().map(|m| m.token.as_str()).collect();
        assert_eq!(
            tokens,
            vec!["q-r1-1", "q-r1-2", "q-r1-3", "q-r1-4", "q-r1-live"],
            "every listed member, abandoned included, oldest first"
        );
        assert_eq!(
            run.matches[0].games,
            vec![Some(true), Some(false), Some(true)]
        );
        let live = &run.matches[4];
        assert_eq!(live.state, MatchState::InProgress);
        assert_eq!(live.games, vec![Some(true), None]);
        assert_eq!(live.sideboarding_events.len(), 1);
        assert_eq!(live.sideboarding_events[0].0, (log, 20));
        assert_eq!(live.newest_game_start, Some((log, 30)));
        assert!(run.matches[0].sideboarding_events.is_empty());
        assert_eq!(run.matches[0].newest_game_start, None);
        assert_eq!(
            run.deck,
            Some(OverlayDeck {
                id: 1,
                name: "Qdeck one".to_string(),
                archetype: None,
                color_identity: Some("G,R,W".to_string()),
                label: Some("Qlabel".to_string()),
                cover_art_crop: Some("https://art/5001.jpg".to_string()),
                cover_local_art_crop: Some("5001_art.jpg".to_string()),
                cover_file: None,
            })
        );
    }

    #[test]
    fn league_card_inputs_keep_active_runs_and_runs_ended_within_the_grace_window() {
        let (_dir, mut conn) = db("overlay-league-selection");
        let account = active_run(&mut conn);
        insert_deck(&conn, 2, "Qdeck two", None);
        store_views(
            &mut conn,
            vec![
                member("q-r2", 100, MatchState::Complete, Outcome::Win),
                member("q-r3", 200, MatchState::Complete, Outcome::Win),
                member("q-r4", 300, MatchState::Complete, Outcome::Win),
                member("q-r5", 400, MatchState::Complete, Outcome::Win),
                member("q-other-account", 450, MatchState::Complete, Outcome::Win),
            ],
        );
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qsecond', 0)",
            [],
        )
        .expect("second account");
        let second = number(&conn, "SELECT id FROM accounts WHERE username = 'Qsecond'");
        let ended = |id, state, completed_at, dropped_at| Run {
            id,
            state,
            started_at: id * 10,
            completed_at,
            dropped_at,
            deck_id: None,
            deck_version_id: Some(2),
        };
        // Complete just inside the window, dropped just outside, partial,
        // and an empty active run.
        insert_run(
            &conn,
            account,
            &ended(2, "complete", Some(NOW - GRACE_MS), None),
        );
        insert_run(
            &conn,
            account,
            &ended(3, "dropped", None, Some(NOW - GRACE_MS - 1)),
        );
        insert_run(&conn, account, &ended(4, "partial", None, None));
        insert_run(&conn, account, &ended(5, "dropped", None, Some(NOW - 1)));
        insert_run(&conn, account, &ended(6, "active", None, None));
        insert_run(&conn, second, &ended(7, "active", None, None));
        join_run(&conn, 2, "q-r2");
        join_run(&conn, 3, "q-r3");
        join_run(&conn, 4, "q-r4");
        join_run(&conn, 5, "q-r5");
        join_run(&conn, 7, "q-other-account");

        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), vec![1, 5, 2], "newest started first");
        // A shorter finished time leaves run 2 out.
        let shorter = league_card_inputs(&conn, account, NOW, finished_ms(1)).expect("read");
        assert_eq!(run_ids(&shorter), vec![1, 5]);
        let complete = input.runs.iter().find(|run| run.id == 2).expect("run 2");
        assert_eq!(complete.completed_at_ms, Some(NOW - GRACE_MS));
        assert_eq!(
            complete.deck.as_ref().map(|deck| deck.id),
            Some(2),
            "the deck of the run's version"
        );
        assert_eq!(
            complete
                .deck
                .as_ref()
                .and_then(|deck| deck.cover_art_crop.clone()),
            None
        );

        // A hidden member leaves the run, and an emptied run is not read.
        hide(&conn, "q-r5");
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), vec![1, 2]);

        // A deleted run is not read either.
        conn.execute("UPDATE leagues SET removed_at = 9 WHERE id = 2", [])
            .expect("removed");
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), vec![1]);
    }

    /// The card shows the status the player set (`user_state`), as the
    /// Leagues page does, not only the fold's: a run the player marked
    /// Complete while the fold still has it Active leaves the card once its
    /// finished time from the edit is over.
    #[test]
    fn league_card_inputs_read_the_status_the_player_set() {
        let (_dir, mut conn) = db("overlay-league-user-state");
        let account = active_run(&mut conn);
        let set = |state: Option<&str>, edited_at: i64| {
            conn.execute(
                "UPDATE leagues SET user_state = ?1, edited_at = ?2 WHERE id = 1",
                params![state, edited_at],
            )
            .expect("status");
        };

        set(Some("complete"), NOW - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        let run = input
            .runs
            .first()
            .expect("run 1 is still in its finished time");
        assert_eq!(run.state, leagues::RunState::Complete);
        assert_eq!(run.completed_at_ms, Some(NOW - 1), "ended when marked");

        set(Some("complete"), NOW - GRACE_MS - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), Vec::<i64>::new(), "past its finished time");

        set(Some("partial"), NOW - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), Vec::<i64>::new(), "partial never shows");

        // A drop the player set, before the fold has taken it: it stays for
        // the finished time from the drop, then leaves.
        let drop = |at: i64| {
            conn.execute(
                "UPDATE leagues SET user_state = 'dropped', user_dropped_at = ?1, edited_at = ?1
                 WHERE id = 1",
                [at],
            )
            .expect("drop");
        };
        drop(NOW - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        let run = input.runs.first().expect("run 1 is in its finished time");
        assert_eq!(run.state, leagues::RunState::Dropped);
        assert_eq!(
            run.dropped_at_ms,
            Some(NOW - 1),
            "dropped when the player dropped it"
        );
        drop(NOW - GRACE_MS - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(run_ids(&input), Vec::<i64>::new(), "past its finished time");
        conn.execute("UPDATE leagues SET user_dropped_at = NULL WHERE id = 1", [])
            .expect("clear the drop time");

        // A run the fold completed keeps its own end; marked Active it
        // stays complete (an old run never becomes the current one).
        conn.execute(
            "UPDATE leagues SET state = 'complete', completed_at = ?1 WHERE id = 1",
            [NOW - 2],
        )
        .expect("fold complete");
        set(Some("complete"), NOW - GRACE_MS - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        let run = input
            .runs
            .first()
            .expect("the fold's end is in its finished time");
        assert_eq!(run.completed_at_ms, Some(NOW - 2));
        set(Some("active"), NOW - 1);
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        let run = input.runs.first().expect("run 1");
        assert_eq!(run.state, leagues::RunState::Complete);
        assert_eq!(run.completed_at_ms, Some(NOW - 2));
    }

    #[test]
    fn a_run_without_a_deck_takes_its_first_versioned_matchs_deck() {
        let (_dir, mut conn) = db("overlay-league-member-deck");
        insert_deck(&conn, 3, "Qdeck three", None);
        insert_deck(&conn, 4, "Qdeck four", None);
        store_views(
            &mut conn,
            vec![
                member("q-m-1", 100, MatchState::Abandoned, Outcome::Unknown),
                member("q-m-2", 200, MatchState::Complete, Outcome::Win),
                member("q-m-3", 300, MatchState::Complete, Outcome::Win),
            ],
        );
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 4 WHERE token = 'q-m-1';
             UPDATE matches SET deck_version_id = 3 WHERE token IN ('q-m-2', 'q-m-3');",
        )
        .expect("links");
        let account = account_id(&conn);
        insert_run(
            &conn,
            account,
            &Run {
                id: 9,
                state: "active",
                started_at: 50,
                completed_at: None,
                dropped_at: None,
                deck_id: None,
                deck_version_id: None,
            },
        );
        for token in ["q-m-1", "q-m-2", "q-m-3"] {
            join_run(&conn, 9, token);
        }
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(
            input.runs[0].deck.as_ref().map(|deck| deck.id),
            Some(3),
            "an abandoned match's deck is skipped"
        );
    }

    /// 0.x "stays within a query budget while in game" (10 queries): the
    /// reads are a fixed number of queries, however many runs, matches and
    /// games there are.
    #[test]
    fn the_league_card_inputs_take_a_bounded_number_of_queries() {
        let (_dir, mut conn) = db("overlay-league-queries");
        let account = active_run(&mut conn);
        let log = file(&conn, "Q/mtgo.log");
        store_events(
            &mut conn,
            vec![
                frame(log, 10, 802, "g first"),
                frame(log, 20, 802, "g newest"),
            ],
        );
        let mut queries = 0;
        let small =
            league_card_inputs_counted(&conn, account, NOW, GRACE_MS, &mut queries).expect("read");
        assert_eq!(small.runs.len(), 1);
        assert!(queries <= 11, "{queries} queries");
        let small_count = queries;

        // One league run and two tournaments past round 1, with matches.
        for (event, token) in [(700, "q-t700"), (800, "q-t800")] {
            store_tournament(&conn, account, event, Some(3));
            let mut views = Vec::new();
            for round in 1..4u32 {
                let mut played = member(
                    &format!("{token}-r{round}"),
                    100 + i64::from(round),
                    MatchState::Complete,
                    Outcome::Win,
                );
                played.tournament_event_id = Some(event as u64);
                played.tournament_round = Some(round);
                views.push(played);
            }
            store_views(&mut conn, views);
            // Mid-event matches have their deck: no format deck is read.
            conn.execute(
                "UPDATE matches SET deck_version_id = 1 WHERE tournament_event_id = ?1",
                [event],
            )
            .expect("decks");
        }
        let mut queries = 0;
        let with_tournaments =
            league_card_inputs_counted(&conn, account, NOW, GRACE_MS, &mut queries).expect("read");
        assert_eq!(with_tournaments.tournaments.len(), 2);
        assert!(
            with_tournaments
                .tournaments
                .iter()
                .all(|tournament| tournament.matches.len() == 3)
        );
        assert_eq!(queries, small_count, "tournaments add no query");
        assert!(queries <= 11, "{queries} queries");

        for run in 20..25 {
            let mut views = Vec::new();
            for index in 0..5 {
                // Older than run 1's live match, so run 1 stays on the card.
                let mut member = member(
                    &format!("q-big-{run}-{index}"),
                    run * 10 + index,
                    MatchState::InProgress,
                    Outcome::Unknown,
                );
                member.games = vec![
                    game(1, Some((run * 100 + index) as u64), GameResult::Win),
                    game(2, None, GameResult::Loss),
                ];
                views.push(member);
            }
            store_views(&mut conn, views);
            let frames = (0..5i64)
                .map(|index| frame(log, 100 + index as u64, (run * 100 + index) as u64, "big"))
                .collect();
            store_events(&mut conn, frames);
            insert_run(
                &conn,
                account,
                &Run {
                    id: run,
                    state: "active",
                    started_at: run,
                    completed_at: None,
                    dropped_at: None,
                    deck_id: Some(1),
                    deck_version_id: None,
                },
            );
            for index in 0..5 {
                join_run(&conn, run, &format!("q-big-{run}-{index}"));
            }
        }
        let mut queries = 0;
        let big =
            league_card_inputs_counted(&conn, account, NOW, GRACE_MS, &mut queries).expect("read");
        assert_eq!(big.runs.len(), 6);
        assert_eq!(queries, small_count, "the same queries for 6 runs");
    }

    #[test]
    fn the_last_played_deck_skips_a_deleted_match() {
        let (_dir, mut conn) = db("overlay-last-played-hidden");
        insert_deck(&conn, 1, "Qolder", None);
        insert_deck(&conn, 2, "Qnewer", None);
        store_views(
            &mut conn,
            vec![
                member("q-lh-1", 1_000, MatchState::Complete, Outcome::Win),
                member("q-lh-2", 2_000, MatchState::Complete, Outcome::Win),
            ],
        );
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 1 WHERE token = 'q-lh-1';
             UPDATE matches SET deck_version_id = 2 WHERE token = 'q-lh-2';
             UPDATE matches SET hidden_at = 1 WHERE token = 'q-lh-2';",
        )
        .expect("links");
        let deck = last_played_deck(&conn).expect("read").expect("a deck");
        assert_eq!(deck.id, 1);
    }

    #[test]
    fn the_last_played_deck_skips_arena_matches() {
        let (_dir, mut conn) = db("overlay-last-played-arena");
        insert_deck(&conn, 1, "Qmtgo", None);
        insert_deck(&conn, 2, "Qarena", None);
        store_views(
            &mut conn,
            vec![
                member("q-la-1", 1_000, MatchState::Complete, Outcome::Win),
                member("q-la-2", 2_000, MatchState::Complete, Outcome::Win),
            ],
        );
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 1 WHERE token = 'q-la-1';
             UPDATE matches SET deck_version_id = 2 WHERE token = 'q-la-2';",
        )
        .expect("links");
        crate::test_support::move_match_to_arena(&conn, "q-la-2");
        let deck = last_played_deck(&conn).expect("read").expect("a deck");
        assert_eq!(deck.id, 1);
    }

    #[test]
    fn the_idle_card_takes_the_deck_of_the_newest_match_with_one() {
        let (_dir, mut conn) = db("overlay-last-played");
        assert_eq!(last_played_deck(&conn).expect("read"), None);
        insert_card(&conn, 5002, "Qcover two");
        insert_deck(&conn, 1, "Qolder", None);
        insert_deck(&conn, 2, "Qnewer", Some(5002));
        store_views(
            &mut conn,
            vec![
                member("q-lp-1", 1_000, MatchState::Complete, Outcome::Win),
                member("q-lp-2", 2_000, MatchState::Complete, Outcome::Win),
                member("q-lp-3", 3_000, MatchState::Complete, Outcome::Win),
            ],
        );
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 1 WHERE token = 'q-lp-1';
             UPDATE matches SET deck_version_id = 2 WHERE token = 'q-lp-2';",
        )
        .expect("links");
        conn.execute("UPDATE decks SET cover_file = '2-9.png' WHERE id = 2", [])
            .expect("custom cover");
        let deck = last_played_deck(&conn).expect("read").expect("a deck");
        assert_eq!(deck.id, 2);
        assert_eq!(deck.name, "Qnewer");
        assert_eq!(deck.cover_file.as_deref(), Some("2-9.png"));
        assert_eq!(deck.cover_art_crop.as_deref(), Some("https://art/5002.jpg"));
        assert_eq!(deck.cover_local_art_crop.as_deref(), Some("5002_art.jpg"));
        assert_eq!(deck.archetype, None);

        let input = league_card_inputs(&conn, account_id(&conn), NOW, GRACE_MS).expect("read");
        assert!(input.runs.is_empty());
        assert_eq!(input.last_played_deck.map(|deck| deck.id), Some(2));
    }

    #[test]
    fn a_deck_versions_cards_are_read_from_its_signature() {
        let (_dir, conn) = db("overlay-version-cards");
        insert_deck(&conn, 1, "Qdeck", None);
        let signature = decks::signature(&[
            DeckCard {
                catalog_id: 100,
                quantity: 4,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 200,
                quantity: 2,
                sideboard: true,
            },
        ]);
        conn.execute(
            "UPDATE deck_versions SET signature = ?1 WHERE id = 1",
            [&signature],
        )
        .expect("signature");

        let cards = deck_version_cards(&conn, 1)
            .expect("read")
            .expect("a version");
        assert_eq!(cards, decks::decode_signature(&signature));
        assert!(
            cards
                .iter()
                .any(|card| card.catalog_id == 100 && !card.sideboard)
        );
        assert_eq!(deck_version_cards(&conn, 99).expect("read"), None);
    }

    // Login and cards

    fn login(file: i64, offset: u64, at: i64, text: &str) -> NewEvent {
        let mut event = event_with(file, offset, EventKind::Login, Subjects::default(), text);
        event.logged_at_ms = at;
        event
    }

    #[test]
    fn the_current_login_is_the_newest_login_events_account() {
        let (_dir, mut conn) = db("overlay-current-login");
        assert_eq!(current_login(&conn).expect("read"), None);
        let first = file(&conn, "Q/first.log");
        let second = file(&conn, "Q/second.log");
        conn.execute_batch(
            "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qfirst', 111, 0);
             INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qsecond name', NULL, 0);",
        )
        .expect("accounts");
        store_events(
            &mut conn,
            vec![
                // The newest by time is in the first file.
                login(
                    first,
                    10,
                    5_000,
                    "12:00:00 [INF] (Login|MtGO Login Success) Username: Qfirst (111)",
                ),
                login(
                    second,
                    10,
                    4_000,
                    "12:00:00 [INF] (Login|MtGO Login Success) Username: Qsecond name (222)",
                ),
            ],
        );
        assert_eq!(
            current_login(&conn).expect("read"),
            Some(("Qfirst".to_string(), Some(111)))
        );
        store_events(
            &mut conn,
            vec![login(
                second,
                20,
                6_000,
                "12:00:00 [INF] (Login|MtGO Login Success) Username: Qsecond name (222)",
            )],
        );
        assert_eq!(
            current_login(&conn).expect("read"),
            Some(("Qsecond name".to_string(), None)),
            "the account's login id, never guessed from the line"
        );
        // Within a file the log position decides (#303 F6): a later line
        // whose time reads earlier (a clock slip) is still the newest login.
        store_events(
            &mut conn,
            vec![login(
                second,
                30,
                5_500,
                "12:00:00 [INF] (Login|MtGO Login Success) Username: Qfirst (111)",
            )],
        );
        assert_eq!(
            current_login(&conn).expect("read"),
            Some(("Qfirst".to_string(), Some(111))),
            "the second file's last line"
        );
    }

    #[test]
    fn card_meta_reads_every_asked_row() {
        let (_dir, conn) = db("overlay-card-meta");
        insert_card(&conn, 1001, "Qbear");
        conn.execute("INSERT INTO cards (catalog_id) VALUES (1002)", [])
            .expect("stub");
        let meta = card_meta(&conn, &[1001, 1002, 1003, 1001]).expect("read");
        assert_eq!(meta.cards.len(), 2, "unknown ids are left out");
        assert_eq!(
            meta.get(1001),
            Some(&CardInfo {
                name: Some("Qbear".to_string()),
                oracle_id: Some("oracle-Qbear".to_string()),
                card_type: Some("Creature".to_string()),
                color_identity: Some("G".to_string()),
                image: Some("https://img/1001.jpg".to_string()),
                art_crop: Some("https://art/1001.jpg".to_string()),
                local_image: Some("1001.jpg".to_string()),
                local_art_crop: Some("1001_art.jpg".to_string()),
                mana_cost: None,
            })
        );
        assert_eq!(meta.get(1002), Some(&CardInfo::default()));
        assert_eq!(card_meta(&conn, &[]).expect("read"), CardMeta::default());
    }

    // ---- Plan U task 7: the opponent's archetype, the guide, the card -----

    fn insert_archetype(conn: &Connection, id: i64, name: &str, format: &str) {
        conn.execute(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
             VALUES (?1, 'q-uuid-' || ?1, ?2, ?3, 'downloaded', 1, 1)",
            params![id, name, format],
        )
        .expect("archetype");
    }

    fn seat(conn: &Connection, token: &str, opponent: bool, archetype: i64, manual: bool) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES ((SELECT id FROM matches WHERE token = ?1), ?2, ?3, 0.9, ?4)",
            params![token, opponent, archetype, manual],
        )
        .expect("seat");
    }

    /// 0.x ResolveOverlayOpponentTest "falls back to the last archetype this
    /// opponent was seen on", "ignores what we faced them on in another
    /// format" and "does not count an automatic guess on the live match as
    /// history" (U2 review: the store half of the order).
    #[test]
    fn last_faced_skips_the_live_match_and_other_formats() {
        let (_dir, mut conn) = db("overlay-last-faced");
        let mut live = live_view();
        live.format = Some("CMODERN".to_string());
        store_views(
            &mut conn,
            vec![
                past("q-lf-old", "Qrival", "CMODERN", Outcome::Win, 1_000),
                past("q-lf-new", "Qrival", "CMODERN", Outcome::Loss, 2_000),
                past("q-lf-pauper", "Qrival", "CPAUPER", Outcome::Win, 3_000),
                past("q-lf-other", "Qsomeone", "CMODERN", Outcome::Win, 3_500),
                live,
            ],
        );
        for (id, name) in [
            (101, "Qold"),
            (102, "Qnew"),
            (103, "Qpauper"),
            (104, "Qother"),
            (105, "Qguess"),
        ] {
            insert_archetype(&conn, id, name, "modern");
        }
        seat(&conn, "q-lf-old", true, 101, false);
        seat(&conn, "q-lf-new", true, 102, false);
        // The player's own seat never says what the opponent played.
        seat(&conn, "q-lf-new", false, 104, false);
        seat(&conn, "q-lf-pauper", true, 103, true);
        seat(&conn, "q-lf-other", true, 104, false);
        // An automatic guess on the live match is not history.
        seat(&conn, LIVE, true, 105, false);
        let rival = opponent_id(&conn, "Qrival");
        let live = match_id(&conn, LIVE);
        assert_eq!(
            last_faced_archetype(&conn, rival, Some("CMODERN"), live).expect("read"),
            Some(102)
        );
        assert_eq!(
            last_faced_archetype(&conn, rival, Some("CPAUPER"), live).expect("read"),
            Some(103)
        );
        assert_eq!(
            last_faced_archetype(&conn, rival, Some("CLEGACY"), live).expect("read"),
            None
        );
        let someone = opponent_id(&conn, "Qsomeone");
        assert_eq!(
            last_faced_archetype(&conn, someone, Some("CMODERN"), live).expect("read"),
            Some(104)
        );
    }

    #[test]
    fn only_a_hand_set_opponent_row_is_the_manual_pick() {
        let (_dir, mut conn) = db("overlay-manual-seat");
        store_views(&mut conn, vec![live_view()]);
        insert_archetype(&conn, 101, "Qpicked", "modern");
        let live = match_id(&conn, LIVE);
        assert_eq!(manual_opponent_archetype(&conn, live).expect("read"), None);
        seat(&conn, LIVE, true, 101, false);
        assert_eq!(
            manual_opponent_archetype(&conn, live).expect("read"),
            None,
            "an automatic row is not a pick"
        );
        conn.execute("UPDATE match_archetypes SET manual = 1", [])
            .expect("manual");
        assert_eq!(
            manual_opponent_archetype(&conn, live).expect("read"),
            Some(101)
        );
    }

    #[test]
    fn live_game_inputs_name_the_opponent_and_the_deck() {
        let (_dir, mut conn) = db("overlay-live-names");
        insert_deck(&conn, 3, "Qdeck three", None);
        store_views(&mut conn, vec![live_view()]);
        let inputs = live_game_inputs(&conn, LIVE).expect("read").expect("live");
        assert_eq!(inputs.opponent_username.as_deref(), Some("Qrival"));
        assert_eq!(inputs.deck_id, None);
        conn.execute("UPDATE matches SET deck_version_id = 3", [])
            .expect("link");
        let inputs = live_game_inputs(&conn, LIVE).expect("read").expect("live");
        assert_eq!(inputs.deck_id, Some(3));
    }

    /// 0.x BuildOverlayStateTest "sends the deck archetype for the card
    /// title" and "sends a null archetype before detection" (ruling C3).
    #[test]
    fn the_league_card_names_the_decks_archetype() {
        let (_dir, mut conn) = db("overlay-league-archetype");
        let account = active_run(&mut conn);
        conn.execute(
            "UPDATE matches SET deck_version_id = 1 WHERE token = 'q-r1-1'",
            [],
        )
        .expect("link");
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(input.runs[0].deck.as_ref().expect("deck").archetype, None);
        assert_eq!(
            last_played_deck(&conn)
                .expect("read")
                .expect("deck")
                .archetype,
            None
        );
        insert_archetype(&conn, 107, "Qtron", "modern");
        conn.execute("UPDATE decks SET archetype_id = 107 WHERE id = 1", [])
            .expect("tag");
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(
            input.runs[0]
                .deck
                .as_ref()
                .expect("deck")
                .archetype
                .as_deref(),
            Some("Qtron")
        );
        assert_eq!(
            last_played_deck(&conn)
                .expect("read")
                .expect("deck")
                .archetype
                .as_deref(),
            Some("Qtron")
        );
    }

    /// A stub still named by its uuid (the worker has not filled it yet)
    /// shows no archetype, on the card and on the hosted page (F1).
    #[test]
    fn a_stub_named_by_its_uuid_shows_no_archetype() {
        let (_dir, mut conn) = db("overlay-stub-archetype");
        let account = active_run(&mut conn);
        conn.execute(
            "UPDATE matches SET deck_version_id = 1 WHERE token = 'q-r1-1'",
            [],
        )
        .expect("link");
        insert_archetype(&conn, 108, "q-uuid-108", "modern");
        conn.execute("UPDATE decks SET archetype_id = 108 WHERE id = 1", [])
            .expect("tag");
        let input = league_card_inputs(&conn, account, NOW, GRACE_MS).expect("read");
        assert_eq!(input.runs[0].deck.as_ref().expect("deck").archetype, None);
        assert_eq!(
            last_played_deck(&conn)
                .expect("read")
                .expect("deck")
                .archetype,
            None
        );
    }

    #[test]
    fn the_guide_plan_is_the_pairs_planned_cards() {
        let (_dir, conn) = db("overlay-guide-plan");
        insert_deck(&conn, 1, "Qdeck", None);
        insert_deck(&conn, 2, "Qother deck", None);
        insert_archetype(&conn, 101, "Qblink", "modern");
        assert!(guide_plan(&conn, 1, 101).expect("read").is_empty());
        conn.execute_batch(
            "INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
                 VALUES (1, 1, 101, 1, 1), (2, 2, 101, 1, 1);
             INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
                 VALUES (1, 'o-rip', 'in', 2), (1, 'o-bolt', 'out', 1), (2, 'o-other', 'in', 1);",
        )
        .expect("guides");
        let plan = guide_plan(&conn, 1, 101).expect("read");
        let cards: Vec<(String, archetypes::guides::Direction, u32)> = plan
            .into_iter()
            .map(|card| (card.oracle_id, card.direction, card.quantity))
            .collect();
        assert_eq!(
            cards,
            vec![
                ("o-rip".to_string(), archetypes::guides::Direction::In, 2),
                ("o-bolt".to_string(), archetypes::guides::Direction::Out, 1),
            ]
        );
    }

    /// U2 review: a card the deck's version names without a card row still
    /// has a name when another printing of it is held (0.x `$meta ?? $row`).
    #[test]
    fn cards_by_oracle_take_the_newest_printing() {
        let (_dir, conn) = db("overlay-cards-by-oracle");
        insert_card(&conn, 10, "Qbolt");
        insert_card(&conn, 20, "Qbolt");
        insert_card(&conn, 30, "Qrip");
        let cards = cards_by_oracle(
            &conn,
            &["oracle-Qbolt".to_string(), "oracle-Qnone".to_string()],
        )
        .expect("read");
        assert_eq!(cards.len(), 1);
        let bolt = cards.get("oracle-Qbolt").expect("bolt");
        assert_eq!(bolt.name.as_deref(), Some("Qbolt"));
        assert_eq!(bolt.image.as_deref(), Some("https://img/20.jpg"));
        assert!(cards_by_oracle(&conn, &[]).expect("read").is_empty());
    }

    // Spec 14 (game view budget): the stamp moves with every change to the
    // format's archetypes, so kept candidates are read again after one.
    #[test]
    fn the_candidates_stamp_moves_with_every_archetype_change() {
        let (_dir, conn) = db("overlay-candidates-stamp");
        let stamp = |conn: &Connection| candidates_stamp(conn, "modern").expect("stamp");
        let empty = stamp(&conn);
        conn.execute_batch(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
             VALUES (501, 'q-a', 'Qa', 'modern', 'manual', 5, 5),
                    (502, 'q-b', 'Qb', 'legacy', 'manual', 9, 9);",
        )
        .expect("archetypes");
        let added = stamp(&conn);
        assert_ne!(added, empty);
        conn.execute("UPDATE archetypes SET updated_at = 6 WHERE id = 501", [])
            .expect("touch");
        let touched = stamp(&conn);
        assert_ne!(touched, added, "an edit moves updated_at");
        conn.execute("UPDATE archetypes SET updated_at = 20 WHERE id = 502", [])
            .expect("other format");
        assert_eq!(
            stamp(&conn),
            touched,
            "another format's edit does not count"
        );
        conn.execute("DELETE FROM archetypes WHERE id = 501", [])
            .expect("delete");
        assert_ne!(stamp(&conn), touched);
    }
}
