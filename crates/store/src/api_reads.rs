//! What the API worker reads (07 spec section 8): the tournament events not
//! yet queued as observations, the tournaments whose real name to ask, and
//! the match reports and card stats (ruling 3; archetypes spec section 13:
//! their archetype check is SQL over `match_archetypes`). Read-only; the
//! writes go through the tracker (`outbox::OutboxEdit`).

pub use mymtgo_api::TournamentEvent;
use mymtgo_api::{
    ArchetypeRef, CardStatsCard, CardStatsGame, MatchArchetypes, MatchReportInput, OutboxKind,
    ReportGame,
};
use rusqlite::{OptionalExtension, Row, params};

mod arena;

use crate::card_stats::{FaceNames, deck_row, other_row};
use crate::outbox::pending_count;
use crate::write::decode_raw;
use crate::{Connection, StoreError};

/// Events the enqueue reads per run at most (0.x `limit = 500`).
pub const ENQUEUE_LIMIT: u32 = 500;

/// How long a match waits after it ends (`ended_at`, else its
/// `last_activity_at`) before it is reported, its card stats are queued or
/// it is synced: 5 minutes, so a fix made right after the match goes out
/// in the first send (edit a match spec section 5). Reports and card stats
/// are sent once, so a later fix would never reach them.
pub const SEND_HOLD_MS: i64 = 300_000;

/// The latest end a match may have to be sent at `now_ms` (the
/// `SEND_HOLD_MS` rule's SQL parameter).
pub fn held_until(now_ms: i64) -> i64 {
    now_ms.saturating_sub(SEND_HOLD_MS)
}

/// The tournament events with no `tournament_observation` outbox row, in
/// `(log_file_id, byte_offset)` order through `log_events_tournament_kinds`.
pub const TOURNAMENT_EVENTS_SQL: &str = "SELECT e.log_file_id, e.byte_offset, e.kind, e.raw,
         e.compressed, e.tournament_token, e.logged_at
     FROM log_events e
     WHERE e.kind IN ('tournament_sync', 'tournament_state_changed', 'tournament_round_result',
             'tournament_round_info', 'tournament_player_eliminated', 'tournament_ended')
       AND NOT EXISTS (
           SELECT 1 FROM outbox o
           WHERE o.kind = 'tournament_observation'
             AND o.subject = e.log_file_id || ':' || e.byte_offset)
     ORDER BY e.log_file_id, e.byte_offset
     LIMIT ?1";

/// Up to `limit` unqueued tournament events, their raw text decoded.
pub fn unqueued_tournament_events(
    conn: &Connection,
    limit: u32,
) -> Result<Vec<TournamentEvent>, StoreError> {
    let mut statement = conn.prepare_cached(TOURNAMENT_EVENTS_SQL)?;
    let mut rows = statement.query([limit])?;
    let mut events = Vec::new();
    while let Some(row) = rows.next()? {
        let raw: Vec<u8> = row.get(3)?;
        let compressed: bool = row.get(4)?;
        events.push(TournamentEvent {
            log_file_id: row.get(0)?,
            byte_offset: row.get(1)?,
            kind: row.get(2)?,
            raw: decode_raw(raw, compressed)?,
            tournament_token: row.get(5)?,
            logged_at_ms: row.get(6)?,
        });
    }
    Ok(events)
}

/// The `mtgo_event_id`s of up to `limit` tournaments whose name is still
/// synthesized and was never asked about, newest start first.
pub fn tournaments_to_name(conn: &Connection, limit: u32) -> Result<Vec<i64>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT mtgo_event_id FROM tournaments
         WHERE name_synthesized = 1 AND name_checked_at IS NULL
         ORDER BY started_at DESC, mtgo_event_id DESC
         LIMIT ?1",
    )?;
    let ids = statement.query_map([limit], |row| row.get(0))?;
    let mut all = Vec::new();
    for id in ids {
        all.push(id?);
    }
    Ok(all)
}

/// The matches 0.x would report (`scopeSubmittable` and
/// `SubmitMatchToApi`'s guards), at most `?1`, in start order: complete,
/// with a deck version, not manual, not limited (format null or not `D%`),
/// no `match_report` outbox row yet, ended at least `SEND_HOLD_MS` ago
/// (`?2` is `held_until(now)`), and an archetype on the player seat
/// (`lp`, `pa`). The opponent seat (`lo`, `oa`) is optional. Read with each
/// seat's uuid and source.
const REPORT_CANDIDATES_SQL: &str = "SELECT m.id, pa.uuid, pa.source, oa.uuid, oa.source
     FROM matches m
     JOIN match_archetypes lp ON lp.match_id = m.id AND lp.is_opponent = 0
     JOIN archetypes pa ON pa.id = lp.archetype_id
     LEFT JOIN match_archetypes lo ON lo.match_id = m.id AND lo.is_opponent = 1
     LEFT JOIN archetypes oa ON oa.id = lo.archetype_id
     JOIN accounts ac ON ac.id = m.account_id AND ac.platform = 'mtgo'
     WHERE m.state = 'complete' AND m.deck_version_id IS NOT NULL AND m.source <> 'manual'
       AND (m.format IS NULL OR m.format NOT LIKE 'D%')
       AND COALESCE(m.ended_at, m.last_activity_at) <= ?2
       AND NOT EXISTS (
           SELECT 1 FROM outbox o WHERE o.kind = 'match_report' AND o.subject = m.token)
     ORDER BY m.started_at, m.id
     LIMIT ?1";

/// How many matches `REPORT_CANDIDATES_SQL` would read without a limit (the
/// same joins and rules; `?1` is `held_until(now)`).
const PENDING_REPORTS_SQL: &str = "SELECT COUNT(*)
     FROM matches m
     JOIN match_archetypes lp ON lp.match_id = m.id AND lp.is_opponent = 0
     JOIN archetypes pa ON pa.id = lp.archetype_id
     LEFT JOIN match_archetypes lo ON lo.match_id = m.id AND lo.is_opponent = 1
     LEFT JOIN archetypes oa ON oa.id = lo.archetype_id
     JOIN accounts ac ON ac.id = m.account_id AND ac.platform = 'mtgo'
     WHERE m.state = 'complete' AND m.deck_version_id IS NOT NULL AND m.source <> 'manual'
       AND (m.format IS NULL OR m.format NOT LIKE 'D%')
       AND COALESCE(m.ended_at, m.last_activity_at) <= ?1
       AND NOT EXISTS (
           SELECT 1 FROM outbox o WHERE o.kind = 'match_report' AND o.subject = m.token)";

/// One seat's archetype from a row's `uuid` and `source` columns at `at`
/// and `at + 1`; `None` when the seat has no row.
fn seat_at(row: &Row, at: usize) -> rusqlite::Result<Option<ArchetypeRef>> {
    let uuid: Option<String> = row.get(at)?;
    let source: Option<String> = row.get(at + 1)?;
    // A commander archetype (Arena Brawl) is made on this machine from the
    // commanders: the API has no such uuid, so the seat goes as null.
    if source.as_deref() == Some("commander") {
        return Ok(None);
    }
    Ok(uuid.map(|uuid| {
        let source = source.unwrap_or_default();
        ArchetypeRef {
            uuid,
            manual: source == "manual",
            is_fallback: source == "fallback",
        }
    }))
}

/// Up to `limit` matches to report, each read into its report input
/// (`client_version` is the app's). A match that ended less than
/// `SEND_HOLD_MS` before `now_ms` waits.
pub fn report_candidates(
    conn: &Connection,
    limit: u32,
    client_version: &str,
    now_ms: i64,
) -> Result<Vec<MatchReportInput>, StoreError> {
    let mut found: Vec<(i64, MatchArchetypes)> = Vec::new();
    {
        let mut statement = conn.prepare_cached(REPORT_CANDIDATES_SQL)?;
        let rows = statement.query_map(params![limit, held_until(now_ms)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                MatchArchetypes {
                    player: seat_at(row, 1)?,
                    opponent: seat_at(row, 3)?,
                },
            ))
        })?;
        for row in rows {
            found.push(row?);
        }
    }
    let mut inputs = Vec::new();
    for (id, sides) in found {
        if let Some(input) = report_input(conn, id, client_version, sides)? {
            inputs.push(input);
        }
    }
    // Arena matches follow the MTGO ones and share the limit.
    let room = limit.saturating_sub(inputs.len() as u32);
    if room > 0 {
        inputs.extend(arena::arena_candidates(conn, room, client_version, now_ms)?);
    }
    Ok(inputs)
}

/// The matches waiting to be reported: those to queue plus the queued ones
/// not yet sent (0.x "N matches pending."). A match still in its
/// `SEND_HOLD_MS` at `now_ms` is not counted yet.
pub fn pending_report_count(conn: &Connection, now_ms: i64) -> Result<u32, StoreError> {
    let to_queue: u32 = conn
        .prepare_cached(PENDING_REPORTS_SQL)?
        .query_row([held_until(now_ms)], |row| row.get(0))?;
    let arena_to_queue = arena::arena_pending_to_queue(conn, now_ms)?;
    Ok(to_queue + arena_to_queue + pending_count(conn, OutboxKind::MatchReport)?)
}

/// One match's report input; `None` when the match is gone.
fn report_input(
    conn: &Connection,
    match_id: i64,
    client_version: &str,
    sides: MatchArchetypes,
) -> Result<Option<MatchReportInput>, StoreError> {
    let row = conn
        .prepare_cached(
            "SELECT m.token, a.username, a.login_id, m.outcome, m.format, m.tournament_event_id,
                 l.token, m.league_id, m.tournament_token, m.tournament_round, m.started_at,
                 o.username, v.signature,
                 CASE WHEN l.token IS NOT NULL THEN l.sync_client_id END,
                 CASE WHEN l.source <> 'manual' THEN l.event_id END
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             LEFT JOIN leagues l ON l.id = m.league_id
             LEFT JOIN deck_versions v ON v.id = m.deck_version_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| {
            Ok((
                MatchReportInput {
                    match_token: row.get(0)?,
                    username: row.get(1)?,
                    login_id: row.get(2)?,
                    player_archetype: sides.player.clone(),
                    opponent_archetype: sides.opponent.clone(),
                    outcome: row.get(3)?,
                    format: row.get(4)?,
                    tournament_event_id: row.get(5)?,
                    league_token: row.get(6)?,
                    league_id: row.get(7)?,
                    // The match bundle's rule: the league's stored id, for a
                    // league with a token.
                    league_client_id: row.get(13)?,
                    // A manual league's event id is not MTGO's.
                    league_playevent_id: row.get(14)?,
                    tournament_token: row.get(8)?,
                    tournament_round: row.get(9)?,
                    started_at_ms: row.get(10)?,
                    client_version: client_version.to_string(),
                    opponent_name: row.get(11)?,
                    deck: Vec::new(),
                    oracle_cards: Vec::new(),
                    games: Vec::new(),
                    arena: None,
                },
                row.get::<_, Option<String>>(12)?,
            ))
        })
        .optional()?;
    let Some((mut input, signature)) = row else {
        return Ok(None);
    };
    input.deck = signature
        .as_deref()
        .map(decks::decode_signature)
        .unwrap_or_default();
    input.oracle_cards = oracle_cards(conn, &input.deck)?;
    input.games = report_games(conn, match_id)?;
    Ok(Some(input))
}

/// Every MTGO `cards` row (never an Arena one, which would be sent as an
/// `mtgo_id`) sharing an oracle id with a deck card, in catalog id
/// order (what 0.x's `keyBy('oracle_id')` chose from).
fn oracle_cards(
    conn: &Connection,
    deck: &[mymtgo_api::report::DeckCard],
) -> Result<Vec<(u32, String)>, StoreError> {
    let mut oracles: Vec<String> = Vec::new();
    for card in deck {
        let oracle: Option<String> = conn
            .prepare_cached("SELECT oracle_id FROM cards WHERE catalog_id = ?1")?
            .query_row([card.catalog_id], |row| row.get(0))
            .optional()?
            .flatten();
        if let Some(oracle) = oracle.filter(|oracle| !oracle.is_empty())
            && !oracles.contains(&oracle)
        {
            oracles.push(oracle);
        }
    }
    let mut rows = Vec::new();
    for oracle in &oracles {
        let mut statement = conn.prepare_cached(
            "SELECT catalog_id FROM cards
             WHERE oracle_id = ?1 AND catalog_id <= ?2 ORDER BY catalog_id",
        )?;
        let ids = statement.query_map(rusqlite::params![oracle, cards::MAX_CATALOG_ID], |row| {
            row.get::<_, u32>(0)
        })?;
        for id in ids {
            rows.push((id?, oracle.clone()));
        }
    }
    rows.sort();
    Ok(rows)
}

/// A match's games in game number order, with the local kept hand and the
/// opponent's seen cards.
fn report_games(conn: &Connection, match_id: i64) -> Result<Vec<ReportGame>, StoreError> {
    let mut games: Vec<(i64, ReportGame)> = Vec::new();
    {
        let mut statement = conn.prepare_cached(
            "SELECT id, game_number, turn_count, started_at, ended_at, local_mulligans,
                 local_starting_hand_size, opp_mulligans, on_play, result
             FROM games WHERE match_id = ?1 ORDER BY game_number",
        )?;
        let rows = statement.query_map([match_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                ReportGame {
                    game_number: row.get(1)?,
                    turn_count: row.get(2)?,
                    started_at_ms: row.get(3)?,
                    ended_at_ms: row.get(4)?,
                    mulligans: row.get(5)?,
                    starting_hand_size: row.get(6)?,
                    kept_hand: Vec::new(),
                    opponent_mulligans: row.get(7)?,
                    on_play: row.get(8)?,
                    won: row.get::<_, String>(9)? == "win",
                    opponent_seen: Vec::new(),
                    undecided: matches!(row.get::<_, String>(9)?.as_str(), "draw" | "unknown"),
                },
            ))
        })?;
        for row in rows {
            games.push(row?);
        }
    }
    let mut all = Vec::new();
    for (game_id, mut game) in games {
        game.kept_hand = card_counts(
            conn,
            "SELECT catalog_id, starting_hand FROM game_cards
             WHERE game_id = ?1 AND side = 'local' AND starting_hand > 0 ORDER BY catalog_id",
            game_id,
        )?;
        game.opponent_seen = card_counts(
            conn,
            "SELECT catalog_id, seen FROM game_cards
             WHERE game_id = ?1 AND side = 'opponent' AND seen > 0 ORDER BY catalog_id",
            game_id,
        )?;
        all.push(game);
    }
    Ok(all)
}

fn card_counts(conn: &Connection, sql: &str, game_id: i64) -> Result<Vec<(u32, i64)>, StoreError> {
    let mut statement = conn.prepare_cached(sql)?;
    let rows = statement.query_map([game_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut counts = Vec::new();
    for row in rows {
        counts.push(row?);
    }
    Ok(counts)
}

/// The games 0.x would queue card stats for (`EnqueueCardStats`), at most
/// `?1`, in game id order: decided (won or lost), with an MTGO game id (the
/// subject needs it) and local cards, of a complete, deck-linked,
/// non-manual match that ended at least `SEND_HOLD_MS` ago (`?2` is
/// `held_until(now)`) with an archetype on the player's seat, and no
/// `card_stats` outbox row yet. Read with both seats' uuids (the
/// opponent's null when that seat has no row).
///
/// 0.x read every match with an archetype on either seat
/// (`whereHas('archetypes')`) and then skipped a game without a player
/// archetype, unqueued, so it was read again every run. Here the player
/// seat is a join, so those games never take a place in the batch (Plan T
/// follow-up F23); they are still never shipped. Public so the speed test
/// can time the query alone.
pub const CARD_STATS_CANDIDATES_SQL: &str = "SELECT g.id, m.token, g.mtgo_game_id,
         a.username, m.format, g.result, g.on_play, g.game_number, g.started_at, m.started_at,
         pa.uuid, oa.uuid
     FROM games g
     JOIN matches m ON m.id = g.match_id
     JOIN accounts a ON a.id = m.account_id AND a.platform = 'mtgo'
     JOIN match_archetypes lp ON lp.match_id = m.id AND lp.is_opponent = 0
     JOIN archetypes pa ON pa.id = lp.archetype_id
     LEFT JOIN match_archetypes lo ON lo.match_id = m.id AND lo.is_opponent = 1
     LEFT JOIN archetypes oa ON oa.id = lo.archetype_id
     WHERE g.result IN ('win', 'loss') AND g.mtgo_game_id IS NOT NULL
       AND m.state = 'complete' AND m.deck_version_id IS NOT NULL AND m.source <> 'manual'
       AND COALESCE(m.ended_at, m.last_activity_at) <= ?2
       AND EXISTS (SELECT 1 FROM game_cards c WHERE c.game_id = g.id AND c.side = 'local')
       AND NOT EXISTS (
           SELECT 1 FROM outbox o
           WHERE o.kind = 'card_stats' AND o.subject = m.token || ':' || g.mtgo_game_id)
     ORDER BY g.id
     LIMIT ?1";

/// Up to `limit` games to queue card stats for, each read in full. The
/// payload rules (a player archetype uuid, a format, a date, cards with an
/// oracle id) are `mymtgo_api::card_stats_payload`'s; manual and fallback
/// uuids are sent as stored (0.x). A match that ended less than
/// `SEND_HOLD_MS` before `now_ms` waits.
pub fn card_stats_candidates(
    conn: &Connection,
    limit: u32,
    now_ms: i64,
) -> Result<Vec<CardStatsGame>, StoreError> {
    let mut games: Vec<(i64, CardStatsGame)> = Vec::new();
    {
        let mut statement = conn.prepare_cached(CARD_STATS_CANDIDATES_SQL)?;
        let rows = statement.query_map(params![limit, held_until(now_ms)], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                CardStatsGame {
                    match_token: row.get(1)?,
                    mtgo_game_id: row.get(2)?,
                    local_player: row.get(3)?,
                    format: row.get(4)?,
                    won: row.get::<_, String>(5)? == "win",
                    on_play: row.get(6)?,
                    game_number: row.get(7)?,
                    game_started_at_ms: row.get(8)?,
                    match_started_at_ms: row.get(9)?,
                    player_archetype_uuid: row.get(10)?,
                    opponent_archetype_uuid: row.get(11)?,
                    cards: Vec::new(),
                },
            ))
        })?;
        for row in rows {
            games.push(row?);
        }
    }
    let mut all = Vec::new();
    let mut faces = FaceNames::new();
    for (game_id, mut game) in games {
        game.cards = local_cards(conn, game_id, &mut faces)?;
        all.push(game);
    }
    Ok(all)
}

/// The local side's `game_cards` of a game that 0.x's `card_game_stats`
/// would have had (`card_stats::deck_row`, the card stats tab's rule, issue
/// #295), with their casts and oracle ids, in catalog id order. A row MTGO
/// logged under another printing of a deck card (issue #319) or under a
/// multi-face card's face (issue #320), `card_stats::other_row`, is folded into that card (`fold_variant`), with the card's
/// other printings that game, so the caps are at the oracle's copies as on
/// the card stats tab.
/// `faces` resolves faces with one face map for every game read.
fn local_cards(
    conn: &Connection,
    game_id: i64,
    faces: &mut FaceNames,
) -> Result<Vec<CardStatsCard>, StoreError> {
    let mut cards: Vec<(u32, bool, CardStatsCard)> = Vec::new();
    {
        let sql = format!(
            "SELECT c.catalog_id, k.oracle_id, c.main_qty, c.starting_hand, c.seen, c.hand_seen,
                 c.graveyard_seen, c.exile_seen, c.battlefield_seen, c.discarded, c.cast_turn,
                 c.has_zone_data, c.cast, c.played, c.activated, c.sided_in, c.sided_out,
                 c.pregame_revealed, c.pregame_played, NOT {deck}
             FROM game_cards c
             JOIN games g ON g.id = c.game_id
             JOIN matches m ON m.id = g.match_id
             LEFT JOIN cards k ON k.catalog_id = c.catalog_id
             WHERE c.game_id = ?1 AND c.side = 'local' AND ({deck} OR {other})
             ORDER BY c.catalog_id",
            deck = deck_row("c"),
            other = other_row("c")
        );
        let mut statement = conn.prepare_cached(&sql)?;
        let rows = statement.query_map([game_id], |row| {
            Ok((
                row.get::<_, u32>(0)?,
                row.get::<_, bool>(19)?,
                CardStatsCard {
                    oracle_id: row.get(1)?,
                    quantity: row.get(2)?,
                    kept: row.get(3)?,
                    seen: row.get(4)?,
                    hand_seen: row.get(5)?,
                    graveyard_seen: row.get(6)?,
                    exile_seen: row.get(7)?,
                    battlefield_seen: row.get(8)?,
                    discarded: row.get(9)?,
                    cast_turn: row.get(10)?,
                    has_zone_data: row.get(11)?,
                    cast: row.get(12)?,
                    played: row.get(13)?,
                    activated: row.get(14)?,
                    casts: Vec::new(),
                    sided_in: row.get(15)?,
                    sided_out: row.get(16)?,
                    pregame_revealed: row.get(17)?,
                    pregame_played: row.get(18)?,
                },
            ))
        })?;
        for row in rows {
            cards.push(row?);
        }
    }
    let mut all = Vec::new();
    let mut variants = Vec::new();
    for (catalog_id, variant, mut card) in cards {
        let mut statement = conn.prepare_cached(
            "SELECT method, count FROM game_card_casts
             WHERE game_id = ?1 AND side = 'local' AND catalog_id = ?2 ORDER BY method",
        )?;
        let casts = statement.query_map(params![game_id, catalog_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        for cast in casts {
            card.casts.push(cast?);
        }
        if variant {
            if card.oracle_id.is_none() {
                // A face logged under its own catalog id (issue #320).
                card.oracle_id = faces.oracle(conn, catalog_id)?.map(|(oracle, _)| oracle);
            }
            variants.push(card);
        } else {
            all.push(card);
        }
    }
    let mut folded: Vec<usize> = Vec::new();
    let mut merged_away: Vec<usize> = Vec::new();
    for variant in &variants {
        let Some(oracle) = variant.oracle_id.as_deref() else {
            continue;
        };
        // Folded only into a card of this oracle in the deck that game.
        let target = all
            .iter()
            .position(|card| card.quantity > 0 && card.oracle_id.as_deref() == Some(oracle));
        let Some(index) = target else {
            continue;
        };
        if !folded.contains(&index) {
            folded.push(index);
            merge_printings(&mut all, index, &mut merged_away);
        }
        fold_variant(&mut all[index], variant);
    }
    for index in &folded {
        cap_at_quantity(&mut all[*index]);
    }
    let mut kept = Vec::new();
    for (index, card) in all.into_iter().enumerate() {
        if !merged_away.contains(&index) {
            kept.push(card);
        }
    }
    Ok(kept)
}

/// Add every other row of the card at `index`'s oracle into it, before
/// or after it, as the payload would (`mymtgo_api::add_printing`), and note
/// them in `merged_away`: a variant folded in is then capped at the
/// oracle's copies (the #319 review), as the card stats tab caps it.
fn merge_printings(all: &mut [CardStatsCard], index: usize, merged_away: &mut Vec<usize>) {
    let oracle = all[index].oracle_id.clone();
    for other in 0..all.len() {
        if other != index && all[other].oracle_id == oracle && !merged_away.contains(&other) {
            let printing = all[other].clone();
            mymtgo_api::add_printing(&mut all[index], &printing);
            merged_away.push(other);
        }
    }
}

/// Fold a variant row into its deck card (issue #319), as 0.x counted every
/// printing the player touched under the deck card's oracle. The log
/// counters, casts and kept copies add; the copies seen and the zone counts
/// count instances, and a variant's are the deck card's own under another
/// catalog id, so they take the larger.
fn fold_variant(card: &mut CardStatsCard, variant: &CardStatsCard) {
    card.kept += variant.kept;
    card.seen = card.seen.max(variant.seen);
    card.hand_seen = card.hand_seen.max(variant.hand_seen);
    card.graveyard_seen = card.graveyard_seen.max(variant.graveyard_seen);
    card.exile_seen = card.exile_seen.max(variant.exile_seen);
    card.battlefield_seen = card.battlefield_seen.max(variant.battlefield_seen);
    card.discarded = card.discarded.max(variant.discarded);
    card.cast_turn = match (card.cast_turn, variant.cast_turn) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    card.has_zone_data |= variant.has_zone_data;
    card.cast += variant.cast;
    card.played = add_optional(card.played, variant.played);
    card.activated = add_optional(card.activated, variant.activated);
    for (method, count) in &variant.casts {
        match card.casts.iter_mut().find(|(seen, _)| seen == method) {
            Some((_, total)) => *total += count,
            None => card.casts.push((method.clone(), *count)),
        }
    }
    card.casts.sort();
    card.pregame_revealed += variant.pregame_revealed;
    card.pregame_played += variant.pregame_played;
}

/// 0.x's per oracle caps on a card with a variant folded in: kept, seen and
/// the zone counts at the copies in the deck, and no discard for a card
/// that was cast (a cast spell reaches the graveyard like a discarded one).
fn cap_at_quantity(card: &mut CardStatsCard) {
    let quantity = card.quantity;
    card.kept = card.kept.min(quantity);
    card.seen = card.seen.min(quantity);
    card.hand_seen = card.hand_seen.min(quantity);
    card.graveyard_seen = card.graveyard_seen.min(quantity);
    card.exile_seen = card.exile_seen.min(quantity);
    card.battlefield_seen = card.battlefield_seen.min(quantity);
    card.discarded = if card.cast > 0 {
        0
    } else {
        card.discarded.min(quantity)
    };
}

fn add_optional(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{db, event_with, new_log_file};
    use crate::write::{NewEvent, WriteBatch, commit, insert_log_file};
    use fixture_tools::{load_fixture, prepared_events};
    use mtgo_protocol::{EventKind, Subjects};
    use mymtgo_api::report::ArenaReport;
    use mymtgo_api::{NewOutboxRow, OutboxKind, observation_row};
    use rusqlite::params;

    fn tournament(file: i64, offset: u64, kind: EventKind, raw: &str) -> NewEvent {
        let subjects = Subjects {
            tournament_token: Some("q-tour".to_string()),
            ..Subjects::default()
        };
        event_with(file, offset, kind, subjects, raw)
    }

    fn store_events(conn: &mut Connection, events: Vec<NewEvent>) {
        let batch = WriteBatch {
            events,
            now_ms: 1_000,
            ..WriteBatch::default()
        };
        commit(conn, &batch).expect("commit");
    }

    fn queue(conn: &mut Connection, subject: &str) {
        let rows = vec![NewOutboxRow {
            kind: OutboxKind::TournamentObservation,
            subject: subject.to_string(),
            payload: None,
            now_ms: 1,
        }];
        crate::outbox::edit_outbox(conn, &crate::outbox::OutboxEdit::Enqueue { rows })
            .expect("queued");
    }

    #[test]
    fn unqueued_tournament_events_come_in_file_and_offset_order_decoded() {
        let (_dir, mut conn) = db("api-reads-events");
        let one = insert_log_file(&conn, &new_log_file("/q/one.log")).expect("file");
        let big = format!(
            "Message: {{\"Token\":\"q-tour\",\"Pad\":\"{}\"}}",
            "q".repeat(9_000)
        );
        store_events(
            &mut conn,
            vec![
                tournament(
                    one,
                    30,
                    EventKind::TournamentEnded,
                    "Message: {\"Token\":\"q\"}",
                ),
                tournament(one, 10, EventKind::TournamentSync, &big),
                crate::test_support::token_event(one, 20, "q-match"),
                tournament(one, 40, EventKind::TournamentStateChanged, "state"),
            ],
        );
        let compressed: bool = conn
            .query_row(
                "SELECT compressed FROM log_events WHERE byte_offset = 10",
                [],
                |row| row.get(0),
            )
            .expect("row");
        assert!(compressed, "the big line is stored compressed");
        let events = unqueued_tournament_events(&conn, 500).expect("read");
        let seen: Vec<(i64, &str)> = events
            .iter()
            .map(|event| (event.byte_offset, event.kind.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                (10, "tournament_sync"),
                (30, "tournament_ended"),
                (40, "tournament_state_changed"),
            ]
        );
        assert_eq!(events[0].raw, big, "decoded");
        assert_eq!(events[0].log_file_id, one);
        assert_eq!(events[0].tournament_token.as_deref(), Some("q-tour"));
        assert_eq!(events[0].logged_at_ms, 1_010);

        queue(&mut conn, &format!("{one}:30"));
        let left: Vec<i64> = unqueued_tournament_events(&conn, 500)
            .expect("read")
            .iter()
            .map(|event| event.byte_offset)
            .collect();
        assert_eq!(left, vec![10, 40], "a queued event is not read again");
        let limited = unqueued_tournament_events(&conn, 1).expect("read");
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].byte_offset, 10);
    }

    #[test]
    fn the_event_scan_walks_the_tournament_kinds_index() {
        let (_dir, conn) = db("api-reads-plan");
        let plan: Vec<String> = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {TOURNAMENT_EVENTS_SQL}"))
            .expect("explain")
            .query_map(params![500], |row| row.get::<_, String>(3))
            .expect("plan")
            .collect::<Result<_, _>>()
            .expect("rows");
        let plan = plan.join(" | ");
        assert!(plan.contains("log_events_tournament_kinds"), "{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "{plan}");
        assert!(plan.contains("sqlite_autoindex_outbox"), "{plan}");
    }

    // Observations from a real (anonymised) tournament log build 0.x's
    // payloads: every event of the six kinds, each with a payload.
    #[test]
    fn a_fixture_tournament_log_builds_its_observations() {
        let (_dir, mut conn) = db("api-reads-fixture");
        let fixture = load_fixture("draft_hobbit");
        let file = insert_log_file(&conn, &new_log_file("/q/hobbit.log")).expect("file");
        let events: Vec<NewEvent> = prepared_events(&fixture)
            .into_iter()
            .map(|event| NewEvent {
                log_file_id: file,
                byte_offset: event.offset,
                kind: Some(event.kind),
                subjects: event.subjects,
                logged_at_ms: event.at_ms,
                raw: event.text,
                quarantined: false,
            })
            .collect();
        let mut expected: std::collections::BTreeMap<&str, usize> = Default::default();
        for event in &events {
            let kind = event.kind.expect("classified").as_str();
            if mymtgo_api::OBSERVATION_KINDS.contains(&kind) {
                *expected.entry(kind).or_default() += 1;
            }
        }
        store_events(&mut conn, events);
        let read = unqueued_tournament_events(&conn, 10_000).expect("read");
        let mut counted: std::collections::BTreeMap<&str, usize> = Default::default();
        for event in &read {
            *counted.entry(event.kind.as_str()).or_default() += 1;
        }
        assert_eq!(counted, expected);
        assert_eq!(expected.len(), 6, "every kind: {expected:?}");
        let mut inner_rounds = 0;
        for event in &read {
            let row = observation_row(event, 5);
            assert_eq!(row.subject, format!("{file}:{}", event.byte_offset));
            let payload = row
                .payload
                .unwrap_or_else(|| panic!("{} at {} is skipped", event.kind, event.byte_offset));
            let stored: serde_json::Value =
                serde_json::from_slice(&zstd::decode_all(payload.as_slice()).expect("zstd"))
                    .expect("json");
            let token = event.tournament_token.clone().expect("a token");
            assert_eq!(stored["tournament_token"], token.as_str());
            assert_eq!(stored["event_type"], event.kind.as_str());
            assert_eq!(
                stored["client_observed_at"],
                mymtgo_api::php::iso8601_utc(event.logged_at_ms).as_str()
            );
            let body = &stored["payload"];
            match event.kind.as_str() {
                "tournament_state_changed" => {
                    assert!(body["from"].is_string() && body["to"].is_string(), "{body}");
                    assert!(!body["to"].as_str().unwrap_or("").ends_with(')'), "{body}");
                }
                "tournament_sync" => assert_eq!(body["EventToken"], token.as_str()),
                _ if body.get("Token").is_none() => {
                    // MTGO can log a round info message whose JSON does not
                    // parse; 0.x's first-JSON rule then finds the inner
                    // `Round` object (known traps, "Tournament lines").
                    assert_eq!(event.kind, "tournament_round_info", "{body}");
                    assert!(body["Matches"].is_array(), "{body}");
                    inner_rounds += 1;
                }
                _ => assert_eq!(body["Token"], token.as_str()),
            }
        }
        assert!(
            inner_rounds > 0,
            "the fixture holds the malformed round info"
        );
    }

    #[test]
    fn tournaments_to_name_are_synthesized_unchecked_newest_first() {
        let (_dir, conn) = db("api-reads-names");
        for (event, started, synthesized, checked) in [
            (1, 100, 1, None),
            (2, 300, 1, None),
            (3, 200, 0, None),
            (4, 400, 1, Some(5)),
            (5, 250, 1, None),
        ] {
            conn.execute(
                "INSERT INTO tournaments (mtgo_event_id, name, started_at, name_synthesized,
                     name_checked_at)
                 VALUES (?1, 'Qname', ?2, ?3, ?4)",
                params![event, started, synthesized, checked],
            )
            .expect("tournament");
        }
        assert_eq!(tournaments_to_name(&conn, 50).expect("read"), vec![2, 5, 1]);
        assert_eq!(tournaments_to_name(&conn, 2).expect("read"), vec![2, 5]);
    }

    // ---- Match reports and card stats (ruling 3) ---------------------------

    use mymtgo_api::report_body;

    const VERSION: &str = "1.0.0-beta.2";

    /// A now long after every fixture match's `SEND_HOLD_MS`.
    const AFTER_THE_HOLD: i64 = i64::MAX;
    const PLAYER_UUID: &str = "aaaaaaaa-1111-4111-8111-111111111111";
    const OPPONENT_UUID: &str = "bbbbbbbb-2222-4222-8222-222222222222";
    /// A manual archetype's uuid: the device prefix plus a uuid (0.x
    /// `StoreManualArchetype`).
    const MANUAL_UUID: &str = "a1b2c3d4-cccccccc-3333-4333-8333-333333333333";

    /// One seat's archetype row of the match `token` (detection's, `manual`
    /// 0), replacing the seat's row. A `downloaded` or `manual` archetype is
    /// created in CMODERN when new; the fallbacks are the baseline's seeds.
    fn seat(conn: &Connection, token: &str, is_opponent: bool, uuid: &str, source: &str) {
        if source != "fallback" {
            conn.execute(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES (?1, 'Qarchetype', 'CMODERN', ?2, 0, 0)
                 ON CONFLICT (uuid) DO NOTHING",
                params![uuid, source],
            )
            .expect("archetype");
        }
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES ((SELECT id FROM matches WHERE token = ?1), ?2,
                 (SELECT id FROM archetypes WHERE uuid = ?3), 1.0)
             ON CONFLICT (match_id, is_opponent) DO UPDATE SET archetype_id = excluded.archetype_id",
            params![token, is_opponent, uuid],
        )
        .expect("seat");
    }

    /// Clear one seat of the match `token`.
    fn clear_seat(conn: &Connection, token: &str, is_opponent: bool) {
        conn.execute(
            "DELETE FROM match_archetypes
             WHERE match_id = (SELECT id FROM matches WHERE token = ?1) AND is_opponent = ?2",
            params![token, is_opponent],
        )
        .expect("clear");
    }

    /// 0.x `makeSubmittableMatch`: a complete log match of Qplayer against
    /// Qfoe in CMODERN with a deck version and one game per opponent list
    /// (game N is mtgo game 900+N, won, on the play, kept 1001 x2 and 1002),
    /// and both seats' archetypes (downloaded, as 0.x's factory).
    fn submittable(conn: &Connection, token: &str, opponent_per_game: &[&[(u32, i64)]]) -> i64 {
        conn.execute_batch(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)
                 ON CONFLICT (username) DO NOTHING;
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qfoe', 0)
                 ON CONFLICT (username) DO NOTHING;
             INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES ('900', 1, 'Qdeck', 'Modern', 0, 0, 'file')
                 ON CONFLICT (mtgo_id) DO NOTHING;",
        )
        .expect("registries");
        let signature = decks::signature(&[
            mymtgo_api::report::DeckCard {
                catalog_id: 500,
                quantity: 4,
                sideboard: false,
            },
            mymtgo_api::report::DeckCard {
                catalog_id: 600,
                quantity: 2,
                sideboard: true,
            },
        ]);
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES ((SELECT id FROM decks WHERE mtgo_id = '900'), ?1, 0)
             ON CONFLICT (deck_id, signature) DO NOTHING",
            [&signature],
        )
        .expect("version");
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, state, outcome,
                 started_at, ended_at, last_activity_at, revision, derived_version,
                 deck_version_id)
             VALUES (?1, (SELECT id FROM accounts WHERE username = 'Qplayer'),
                 (SELECT id FROM opponents WHERE username = 'Qfoe'), 'log', 'CMODERN',
                 'complete', 'win', 1787133600000, 1787134600000, 1787134600000, 1, 1,
                 (SELECT id FROM deck_versions))",
            [token],
        )
        .expect("match");
        let match_id = conn.last_insert_rowid();
        for (index, seen) in opponent_per_game.iter().enumerate() {
            let number = index as i64 + 1;
            conn.execute(
                "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                     on_play, local_mulligans, opp_mulligans, local_starting_hand_size,
                     turn_count, started_at, ended_at)
                 VALUES (?1, ?2, ?3, 'win', 'win', 1, 0, 1, 3, 9, ?4, ?5)",
                params![
                    match_id,
                    900 + number,
                    number,
                    1_787_133_600_000 + number * 60_000,
                    1_787_133_600_000 + number * 60_000 + 30_000
                ],
            )
            .expect("game");
            let game_id = conn.last_insert_rowid();
            for (catalog_id, kept) in [(1001, 2), (1002, 1)] {
                conn.execute(
                    "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, opening_hand,
                         starting_hand, seen, cast, sided_out)
                     VALUES (?1, 'local', ?2, 4, ?3, ?3, 1, 1, 1)",
                    params![game_id, catalog_id, kept],
                )
                .expect("local card");
            }
            for (catalog_id, count) in *seen {
                conn.execute(
                    "INSERT INTO game_cards (game_id, side, catalog_id, seen)
                     VALUES (?1, 'opponent', ?2, ?3)",
                    params![game_id, catalog_id, count],
                )
                .expect("opponent card");
            }
        }
        seat(conn, token, false, PLAYER_UUID, "downloaded");
        seat(conn, token, true, OPPONENT_UUID, "downloaded");
        match_id
    }

    fn report_tokens(conn: &Connection) -> Vec<String> {
        report_candidates(conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD)
            .expect("candidates")
            .into_iter()
            .map(|input| input.match_token)
            .collect()
    }

    // 0.x SubmitMatchToApiTest "never reports a limited match" and "leaves
    // limited matches out of the submittable queue".
    #[test]
    fn leaves_limited_matches_out_of_the_report_candidates() {
        let (_dir, conn) = db("api-reads-report-limited");
        submittable(&conn, "q-limited", &[&[(1001, 1)]]);
        conn.execute(
            "UPDATE matches SET format = 'DHOBHOBHOB' WHERE token = 'q-limited'",
            [],
        )
        .expect("limited");
        submittable(&conn, "q-constructed", &[&[(1001, 1)]]);
        assert_eq!(report_tokens(&conn), vec!["q-constructed".to_string()]);
    }

    /// Two submittable matches, `q-mtgo` and `q-arena`; the second belongs to
    /// a new Arena account.
    fn mtgo_and_arena_matches(conn: &Connection) {
        submittable(conn, "q-mtgo", &[&[(1001, 1)]]);
        submittable(conn, "q-arena", &[&[(1001, 1)]]);
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id')",
            [],
        )
        .expect("arena account");
        conn.execute(
            "UPDATE matches SET account_id = (SELECT id FROM accounts WHERE platform = 'arena')
             WHERE token = 'q-arena'",
            [],
        )
        .expect("move");
    }

    /// The Arena match `token` of the account `username`, ended long ago,
    /// game synced and archetype detected: event `Qevent`, Bo1, limited in `S6EOE`, no archetype
    /// seat and no deck version, two games (a win, a loss).
    fn arena_match(conn: &Connection, token: &str, username: &str, display: Option<&str>) {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id, display_name)
             VALUES (?1, 0, 'arena', ?3, ?2) ON CONFLICT (username) DO NOTHING",
            params![
                username,
                display,
                username
                    .strip_prefix("arena:")
                    .map_or(format!("id-{username}"), String::from)
            ],
        )
        .expect("account");
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at, platform, display_name)
             VALUES ('arena:opp-id', 0, 'arena', 'Opp') ON CONFLICT (username) DO NOTHING",
            [],
        )
        .expect("opponent");
        conn.execute(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 game_structure, state, outcome, started_at, ended_at, last_activity_at,
                 revision, derived_version, games_synced_at, archetypes_detected_at)
             VALUES (?1, (SELECT id FROM accounts WHERE username = ?2),
                 (SELECT id FROM opponents WHERE username = 'arena:opp-id'), 'log', 'S6EOE',
                 'limited', 'bo1', 'complete', 'win', 1787133600000, 1787134600000,
                 1787134600000, 1, 1, 1787134601000, 1787134602000)",
            params![token, username],
        )
        .expect("match");
        let match_id = conn.last_insert_rowid();
        for (number, result) in [(1, "win"), (2, "loss")] {
            conn.execute(
                "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                     on_play, local_mulligans, opp_mulligans, local_starting_hand_size,
                     turn_count, started_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4, 'win', 1, 0, 0, 7, 9, 1, 2)",
                params![match_id, 900 + number, number, result],
            )
            .expect("game");
        }
        conn.execute(
            "INSERT INTO arena_match_events (match_token, event_name) VALUES (?1, 'Qevent')",
            [token],
        )
        .expect("event");
    }

    fn arena_inputs(conn: &Connection) -> Vec<MatchReportInput> {
        report_candidates(conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD).expect("candidates")
    }

    /// A reading of `account` on `ladder` at log place (file 1, `offset`).
    fn rank_reading(conn: &Connection, ladder: &str, season: i64, class: &str, offset: i64) {
        conn.execute(
            "INSERT OR IGNORE INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4)",
            [],
        )
        .expect("file");
        conn.execute(
            "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                 class, level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
             VALUES ((SELECT id FROM accounts WHERE username = 'Qr#1'), ?1, ?2, ?5, ?3, 2, 3,
                 0, 0, 1, ?4)",
            params![ladder, season, class, offset, offset * 10],
        )
        .expect("reading");
    }

    /// An Arena match `q-r` of `Qr#1` of `match_type`, first stored event at
    /// log place (file 1, byte 1000).
    fn rank_match(conn: &Connection, match_type: &str) {
        arena_match(conn, "q-r", "Qr#1", None);
        conn.execute(
            "UPDATE matches SET match_type = ?1 WHERE token = 'q-r'",
            [match_type],
        )
        .expect("type");
        conn.execute(
            "INSERT OR IGNORE INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4)",
            [],
        )
        .expect("file");
        conn.execute(
            "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind, match_token,
                 logged_at, raw)
             VALUES (1, 1000, 0, 'room_state', 'q-r', 1, x'')",
            [],
        )
        .expect("event");
    }

    fn the_arena(conn: &Connection) -> ArenaReport {
        arena_inputs(conn)
            .into_iter()
            .find(|input| input.match_token == "q-r")
            .expect("candidate")
            .arena
            .expect("arena")
    }

    #[test]
    fn a_ranked_match_reports_its_stored_reading() {
        let (_dir, conn) = db("api-reads-rank-ranked");
        rank_match(&conn, "ranked");
        rank_reading(&conn, "constructed", 95, "Gold", 100);
        rank_reading(&conn, "constructed", 95, "Platinum", 200);
        conn.execute(
            "UPDATE matches SET rank_file_id = 1, rank_byte_offset = 100 WHERE token = 'q-r'",
            [],
        )
        .expect("stored");
        let arena = the_arena(&conn);
        let rank = arena.rank.expect("rank");
        assert_eq!(rank.ladder, "constructed");
        assert_eq!(rank.class, "Gold");
        assert_eq!((rank.level, rank.step, rank.season), (2, 3, 95));
        assert_eq!(rank.observed_at_ms, 1000);
        assert_eq!(arena.season, Some(95));
    }

    #[test]
    fn a_limited_match_reports_the_limited_ladder() {
        let (_dir, conn) = db("api-reads-rank-limited");
        rank_match(&conn, "limited");
        rank_reading(&conn, "limited", 95, "Bronze", 100);
        rank_reading(&conn, "constructed", 95, "Gold", 200);
        rank_reading(&conn, "limited", 95, "Silver", 300);
        rank_reading(&conn, "limited", 95, "Gold", 2000);
        let rank = the_arena(&conn).rank.expect("rank");
        assert_eq!(rank.ladder, "limited");
        assert_eq!(rank.class, "Silver");
    }

    #[test]
    fn an_unranked_match_has_no_rank_but_a_season() {
        let (_dir, conn) = db("api-reads-rank-unranked");
        rank_match(&conn, "unranked");
        rank_reading(&conn, "constructed", 94, "Gold", 100);
        rank_reading(&conn, "limited", 95, "Bronze", 200);
        let arena = the_arena(&conn);
        assert_eq!(arena.rank, None);
        assert_eq!(arena.season, Some(95));
    }

    #[test]
    fn the_newest_season_wins() {
        let (_dir, conn) = db("api-reads-rank-season");
        rank_match(&conn, "limited");
        rank_reading(&conn, "limited", 95, "Gold", 100);
        rank_reading(&conn, "limited", 94, "Silver", 200);
        let arena = the_arena(&conn);
        assert_eq!(arena.season, Some(95));
        assert_eq!(arena.rank.expect("rank").class, "Gold");
    }

    #[test]
    fn another_accounts_reading_is_not_the_matchs_rank() {
        let (_dir, conn) = db("api-reads-rank-other-account");
        rank_match(&conn, "ranked");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qother#2', 0, 'arena', 'id-other')",
            [],
        )
        .expect("other account");
        rank_reading(&conn, "constructed", 95, "Gold", 100);
        conn.execute(
            "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                 class, level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
             VALUES ((SELECT id FROM accounts WHERE username = 'Qother#2'), 'constructed', 96,
                 2000, 'Mythic', 1, 1, 0, 0, 1, 200)",
            [],
        )
        .expect("other reading");
        let arena = the_arena(&conn);
        assert_eq!(arena.rank.expect("rank").class, "Gold");
        assert_eq!(arena.season, Some(95));
    }

    #[test]
    fn a_reading_at_the_matchs_exact_place_is_not_before_it() {
        let (_dir, conn) = db("api-reads-rank-exact-place");
        rank_match(&conn, "limited");
        rank_reading(&conn, "limited", 95, "Silver", 100);
        rank_reading(&conn, "limited", 95, "Gold", 1000);
        assert_eq!(the_arena(&conn).rank.expect("rank").class, "Silver");
    }

    #[test]
    fn readings_are_placed_across_log_files() {
        let (_dir, conn) = db("api-reads-rank-two-files");
        rank_match(&conn, "limited");
        conn.execute(
            "INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (2, '/arena/Player-prev.log', 5, 6, 7, 8)",
            [],
        )
        .expect("second file");
        // File 1, byte 5000 is after the match's event at file 1, byte 1000.
        rank_reading(&conn, "limited", 95, "Silver", 100);
        rank_reading(&conn, "limited", 95, "Gold", 5000);
        assert_eq!(the_arena(&conn).rank.expect("rank").class, "Silver");
        // The match's event moved to file 2: every file 1 reading is before it.
        conn.execute(
            "UPDATE arena_log_events SET arena_log_file_id = 2 WHERE match_token = 'q-r'",
            [],
        )
        .expect("moved");
        assert_eq!(the_arena(&conn).rank.expect("rank").class, "Gold");
    }

    #[test]
    fn a_ranked_match_with_no_stored_reading_uses_the_latest_before_it() {
        let (_dir, conn) = db("api-reads-rank-null-columns");
        rank_match(&conn, "ranked");
        let stored: (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT rank_file_id, rank_byte_offset FROM matches WHERE token = 'q-r'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("columns");
        assert_eq!(stored, (None, None));
        rank_reading(&conn, "constructed", 95, "Gold", 100);
        rank_reading(&conn, "constructed", 95, "Platinum", 200);
        assert_eq!(the_arena(&conn).rank.expect("rank").class, "Platinum");
    }

    #[test]
    fn no_reading_before_the_match_gives_none() {
        let (_dir, conn) = db("api-reads-rank-none");
        rank_match(&conn, "ranked");
        rank_reading(&conn, "constructed", 95, "Gold", 2000);
        let arena = the_arena(&conn);
        assert_eq!(arena.rank, None);
        assert_eq!(arena.season, None);
    }

    #[test]
    fn arena_matches_are_report_candidates() {
        let (_dir, conn) = db("api-reads-report-arena");
        mtgo_and_arena_matches(&conn);
        assert_eq!(
            report_tokens(&conn).first().map(String::as_str),
            Some("q-mtgo")
        );
        arena_match(&conn, "q-arena-new", "Qnew#1", None);
        let inputs = arena_inputs(&conn);
        let input = inputs
            .iter()
            .find(|input| input.match_token == "q-arena-new")
            .expect("arena candidate");
        let arena = input.arena.as_ref().expect("arena");
        assert_eq!(arena.event_name, "Qevent");
        assert_eq!(arena.best_of, "bo1");
        assert_eq!(arena.match_type, "limited");
        assert_eq!(arena.arena_user_id.as_deref(), Some("id-Qnew#1"));
        assert_eq!(input.player_archetype, None);
        assert_eq!(input.opponent_archetype, None);
        assert!(input.deck.is_empty());
        assert_eq!(input.opponent_name.as_deref(), Some("Opp"));
    }

    /// Links the Arena match `token` to a run `arena:<course>` (as the run
    /// sync does).
    fn link_to_run(conn: &Connection, token: &str, course: &str) {
        conn.execute(
            "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             SELECT ?2, account_id, 'log', 'draft', 'active', ?2, 'Qrun', 1, 1
             FROM matches WHERE token = ?1",
            params![token, format!("arena:{course}")],
        )
        .expect("run");
        conn.execute(
            "UPDATE matches SET league_id = last_insert_rowid() WHERE token = ?1",
            [token],
        )
        .expect("link");
    }

    /// A course snapshot of `course` read at `observed_at` for the account of
    /// the match `token`.
    fn course_snapshot(conn: &Connection, token: &str, course: &str, at: i64, cards: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4)",
            [],
        )
        .expect("file");
        conn.execute(
            "INSERT INTO arena_course_observations (arena_log_file_id, byte_offset, observed_at,
                 account_id, course_id, event_name, module, wins, losses, deck_id, deck_name,
                 cards)
             SELECT 1, ?3, ?3, account_id, ?2, 'Qevent', 'Qmodule', 0, 0, 'd', 'Qdeck', ?4
             FROM matches WHERE token = ?1",
            params![token, course, at, cards],
        )
        .expect("course");
    }

    const POOL_AND_DECK: &str =
        r#"{"main":[[100090001,2]],"side":[[100090002,1]],"pool":[100090001,100090002,100090003]}"#;

    fn deck_of(conn: &Connection, token: &str) -> Vec<decks::DeckCard> {
        arena_inputs(conn)
            .into_iter()
            .find(|input| input.match_token == token)
            .expect("candidate")
            .deck
    }

    fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> decks::DeckCard {
        decks::DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    #[test]
    fn a_limited_match_reports_its_course_deck() {
        let (_dir, conn) = db("api-reads-report-arena-course");
        arena_match(&conn, "q-c", "Qc#1", None);
        link_to_run(&conn, "q-c", "c1");
        course_snapshot(
            &conn,
            "q-c",
            "c1",
            1_787_134_000_000,
            r#"{"main":[[100090009,1]],"side":[],"pool":[]}"#,
        );
        course_snapshot(&conn, "q-c", "c1", 1_787_134_500_000, POOL_AND_DECK);
        assert_eq!(
            deck_of(&conn, "q-c"),
            vec![card(100_090_001, 2, false), card(100_090_002, 1, true)]
        );
    }

    #[test]
    fn a_course_observed_after_the_match_is_ignored() {
        let (_dir, conn) = db("api-reads-report-arena-course-late");
        arena_match(&conn, "q-l", "Ql#1", None);
        link_to_run(&conn, "q-l", "c1");
        course_snapshot(
            &conn,
            "q-l",
            "c1",
            1_787_134_000_000,
            r#"{"main":[[100090009,1]],"side":[],"pool":[]}"#,
        );
        course_snapshot(&conn, "q-l", "c1", 1_787_134_700_000, POOL_AND_DECK);
        assert_eq!(deck_of(&conn, "q-l"), vec![card(100_090_009, 1, false)]);
    }

    #[test]
    fn a_limited_match_without_a_course_reports_an_empty_deck() {
        let (_dir, conn) = db("api-reads-report-arena-no-course");
        arena_match(&conn, "q-n", "Qn#1", None);
        link_to_run(&conn, "q-n", "Qevent:q-n");
        course_snapshot(&conn, "q-n", "Qevent:q-n", 1_787_134_500_000, POOL_AND_DECK);
        assert!(deck_of(&conn, "q-n").is_empty());
        arena_match(&conn, "q-u", "Qu#1", None);
        assert!(deck_of(&conn, "q-u").is_empty());
    }

    #[test]
    fn a_constructed_match_without_a_deck_version_reports_an_empty_deck() {
        let (_dir, conn) = db("api-reads-report-arena-constructed");
        arena_match(&conn, "q-k", "Qk#1", None);
        conn.execute(
            "UPDATE matches SET match_type = 'ranked' WHERE token = 'q-k'",
            [],
        )
        .expect("type");
        link_to_run(&conn, "q-k", "c1");
        course_snapshot(&conn, "q-k", "c1", 1_787_134_500_000, POOL_AND_DECK);
        assert!(deck_of(&conn, "q-k").is_empty());
    }

    #[test]
    fn a_hidden_match_in_a_removed_run_still_reports_its_course_deck() {
        let (_dir, conn) = db("api-reads-report-arena-course-hidden");
        arena_match(&conn, "q-h", "Qh#1", None);
        link_to_run(&conn, "q-h", "c1");
        course_snapshot(&conn, "q-h", "c1", 1_787_134_500_000, POOL_AND_DECK);
        conn.execute("UPDATE leagues SET removed_at = 5", [])
            .expect("removed");
        conn.execute("UPDATE matches SET hidden_at = 5 WHERE token = 'q-h'", [])
            .expect("hidden");
        assert_eq!(
            deck_of(&conn, "q-h"),
            vec![card(100_090_001, 2, false), card(100_090_002, 1, true)]
        );
    }

    #[test]
    fn a_malformed_course_snapshot_gives_an_empty_deck() {
        let (_dir, conn) = db("api-reads-report-arena-course-bad");
        arena_match(&conn, "q-b", "Qb#1", None);
        link_to_run(&conn, "q-b", "c1");
        course_snapshot(&conn, "q-b", "c1", 1_787_134_500_000, "not json");
        assert!(deck_of(&conn, "q-b").is_empty());
    }

    #[test]
    fn equal_snapshot_times_fall_to_the_later_log_place() {
        let (_dir, conn) = db("api-reads-report-arena-course-tie");
        arena_match(&conn, "q-t", "Qt#1", None);
        link_to_run(&conn, "q-t", "c1");
        let at = 1_787_134_400_000;
        course_snapshot(
            &conn,
            "q-t",
            "c1",
            at,
            r#"{"main":[[100090009,1]],"side":[],"pool":[]}"#,
        );
        conn.execute("UPDATE arena_course_observations SET byte_offset = 7", [])
            .expect("offset");
        course_snapshot(&conn, "q-t", "c1", at, POOL_AND_DECK);
        conn.execute(
            "UPDATE arena_course_observations SET byte_offset = 9 WHERE byte_offset = ?1",
            [at],
        )
        .expect("offset");
        assert_eq!(
            deck_of(&conn, "q-t"),
            vec![card(100_090_001, 2, false), card(100_090_002, 1, true)]
        );
    }

    #[test]
    fn the_latest_log_place_wins_over_the_timestamp() {
        let (_dir, conn) = db("api-reads-report-arena-course-order");
        arena_match(&conn, "q-o", "Qo#1", None);
        link_to_run(&conn, "q-o", "c1");
        // The later log place carries the earlier time.
        course_snapshot(
            &conn,
            "q-o",
            "c1",
            1_787_134_500_000,
            r#"{"main":[[100090009,1]],"side":[],"pool":[]}"#,
        );
        conn.execute("UPDATE arena_course_observations SET byte_offset = 1", [])
            .expect("offset");
        course_snapshot(&conn, "q-o", "c1", 1_787_134_400_000, POOL_AND_DECK);
        conn.execute(
            "UPDATE arena_course_observations SET byte_offset = 2 WHERE observed_at = 1787134400000",
            [],
        )
        .expect("offset");
        assert_eq!(
            deck_of(&conn, "q-o"),
            vec![card(100_090_001, 2, false), card(100_090_002, 1, true)]
        );
    }

    fn input_of(conn: &Connection, token: &str) -> MatchReportInput {
        arena_inputs(conn)
            .into_iter()
            .find(|input| input.match_token == token)
            .expect("candidate")
    }

    fn run_keys(input: &MatchReportInput) -> (Option<String>, Option<i64>, Option<String>) {
        (
            input.league_token.clone(),
            input.league_id,
            input.league_client_id.clone(),
        )
    }

    // Spec 4.3: a limited match sends its run's token, row id and sync
    // client id in MTGO's league keys.
    #[test]
    fn a_limited_match_sends_its_run_keys() {
        let (_dir, conn) = db("api-reads-arena-run-keys");
        arena_match(&conn, "q-c", "Qc#1", None);
        link_to_run(&conn, "q-c", "c1");
        conn.execute(
            "UPDATE leagues SET sync_client_id = 'arena:c1_19700101T000000Z'",
            [],
        )
        .expect("client id");
        let run_id: i64 = conn
            .query_row("SELECT id FROM leagues", [], |row| row.get(0))
            .expect("run");
        let input = input_of(&conn, "q-c");
        assert_eq!(
            run_keys(&input),
            (
                Some("arena:c1".to_string()),
                Some(run_id),
                Some("arena:c1_19700101T000000Z".to_string())
            )
        );
        let body = report_body(&input);
        assert_eq!(body["league_token"], serde_json::json!("arena:c1"));
        assert_eq!(body["league_run"], serde_json::json!(run_id));
        assert!(body.get("league_playevent_id").is_none());
    }

    #[test]
    fn a_constructed_match_sends_no_run_keys() {
        let (_dir, conn) = db("api-reads-arena-no-run-keys");
        arena_match(&conn, "q-k", "Qk#1", None);
        conn.execute(
            "UPDATE matches SET match_type = 'ranked' WHERE token = 'q-k'",
            [],
        )
        .expect("type");
        assert_eq!(run_keys(&input_of(&conn, "q-k")), (None, None, None));
    }

    // Ruling: a run keyed by no course (trap 34) says nothing about the
    // entry, so it sends none of the three keys.
    #[test]
    fn an_orphan_run_sends_no_run_keys() {
        let (_dir, conn) = db("api-reads-arena-orphan-run");
        arena_match(&conn, "q-n", "Qn#1", None);
        link_to_run(&conn, "q-n", "Qevent:q-n");
        conn.execute("UPDATE leagues SET sync_client_id = 'q-stale'", [])
            .expect("client id");
        assert_eq!(run_keys(&input_of(&conn, "q-n")), (None, None, None));
    }

    // The match commit comes before its deck link and game sync: a match
    // waits for the game sync's stamp, or a player's fix.
    #[test]
    fn an_arena_match_still_syncing_waits() {
        let (_dir, conn) = db("api-reads-arena-syncing");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        conn.execute("UPDATE matches SET games_synced_at = NULL", [])
            .expect("unsynced");
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
        conn.execute("UPDATE matches SET corrected_at = 5", [])
            .expect("corrected");
        assert_eq!(report_tokens(&conn), vec!["q-a".to_string()]);
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            1
        );
    }

    // #759: a backlog synced at once was reported before detection ran, so
    // the reports carried no archetype. A match detection never takes (a
    // hidden or detached one, or a corrected one with no game sync) is not
    // held back.
    #[test]
    fn an_arena_match_waits_for_archetype_detection() {
        let (_dir, conn) = db("api-reads-arena-detection");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        conn.execute("UPDATE matches SET archetypes_detected_at = NULL", [])
            .expect("undetected");
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
        for never_detected in [
            "UPDATE matches SET hidden_at = 5",
            "UPDATE matches SET detached_at = 5",
            "UPDATE matches SET games_synced_at = NULL, corrected_at = 5",
        ] {
            conn.execute_batch(&format!(
                "UPDATE matches SET hidden_at = NULL, detached_at = NULL,
                     games_synced_at = 1787134601000, corrected_at = NULL;
                 {never_detected}"
            ))
            .expect("update");
            assert_eq!(
                report_tokens(&conn),
                vec!["q-a".to_string()],
                "{never_detected}"
            );
            assert_eq!(
                pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
                1,
                "{never_detected}"
            );
        }
        conn.execute_batch(
            "UPDATE matches SET hidden_at = NULL, detached_at = NULL,
                 games_synced_at = 1787134601000, corrected_at = NULL,
                 archetypes_detected_at = 1787134602000",
        )
        .expect("detected");
        assert_eq!(report_tokens(&conn), vec!["q-a".to_string()]);
    }

    // `arena_username` refuses an empty display name, so the queue must too
    // (else the row is pending forever and holds a batch slot).
    #[test]
    fn an_untagged_account_with_an_empty_display_name_waits() {
        let (_dir, conn) = db("api-reads-arena-empty-display");
        arena_match(&conn, "q-a", "arena:abc-id", Some(""));
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
    }

    // The API refuses a body with no format, a best-of other than Bo1 or
    // Bo3, or another match type, every time: such a match is never queued.
    #[test]
    fn an_arena_match_the_api_refuses_is_not_reported() {
        let (_dir, conn) = db("api-reads-arena-refused");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        let refused = [
            "UPDATE matches SET format = NULL",
            "UPDATE matches SET game_structure = NULL",
            "UPDATE matches SET game_structure = 'bo5'",
            "UPDATE matches SET match_type = NULL",
            "UPDATE matches SET match_type = 'tournament'",
        ];
        for update in refused {
            conn.execute_batch(&format!(
                "UPDATE matches SET format = 'S6EOE', game_structure = 'bo1',
                     match_type = 'limited';
                 {update}"
            ))
            .expect("update");
            assert!(report_tokens(&conn).is_empty(), "{update}");
            assert_eq!(
                pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
                0,
                "{update}"
            );
        }
        conn.execute(
            "UPDATE matches SET match_type = 'ranked', game_structure = 'bo3'",
            [],
        )
        .expect("update");
        conn.execute("UPDATE matches SET format = 'CPIONEER'", [])
            .expect("update");
        assert_eq!(report_tokens(&conn), vec!["q-a".to_string()]);
    }

    #[test]
    fn mtgo_candidates_have_no_arena_part() {
        let (_dir, conn) = db("api-reads-report-mtgo-no-arena");
        mtgo_and_arena_matches(&conn);
        let inputs = arena_inputs(&conn);
        assert!(inputs.iter().all(|input| input.arena.is_none()));
    }

    #[test]
    fn pending_reports_count_arena_matches() {
        let (_dir, conn) = db("api-reads-pending-arena");
        mtgo_and_arena_matches(&conn);
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            1
        );
        arena_match(&conn, "q-arena-new", "Qarena#12345", None);
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            2
        );
    }

    #[test]
    fn an_arena_match_without_an_event_name_is_not_reported() {
        let (_dir, conn) = db("api-reads-arena-no-event");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        conn.execute("DELETE FROM arena_match_events", [])
            .expect("delete");
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
    }

    #[test]
    fn an_unknown_outcome_is_not_reported() {
        let (_dir, conn) = db("api-reads-arena-unknown-outcome");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        conn.execute("UPDATE matches SET outcome = 'unknown'", [])
            .expect("update");
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
        conn.execute("UPDATE matches SET outcome = 'draw'", [])
            .expect("update");
        assert_eq!(report_tokens(&conn), vec!["q-a".to_string()]);
    }

    #[test]
    fn a_manual_arena_match_is_not_reported() {
        let (_dir, conn) = db("api-reads-arena-manual");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        conn.execute("UPDATE matches SET source = 'manual'", [])
            .expect("update");
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
    }

    #[test]
    fn an_arena_match_in_its_hold_waits() {
        let (_dir, conn) = db("api-reads-arena-hold");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        let ended = 1_787_134_600_000;
        let inside = ended + SEND_HOLD_MS - 1;
        assert!(
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, inside)
                .expect("candidates")
                .is_empty()
        );
        assert_eq!(pending_report_count(&conn, inside).expect("count"), 0);
        let outside = ended + SEND_HOLD_MS;
        assert_eq!(
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, outside)
                .expect("candidates")
                .len(),
            1
        );
    }

    #[test]
    fn a_queued_arena_match_is_not_queued_again() {
        let (_dir, conn) = db("api-reads-arena-queued");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        conn.execute(
            "INSERT INTO outbox (kind, subject, payload, status, attempts, created_at)
             VALUES ('match_report', 'q-a', x'7b7d', 'pending', 0, 0)",
            [],
        )
        .expect("outbox");
        assert!(report_tokens(&conn).is_empty());
    }

    #[test]
    fn an_untagged_account_reports_its_display_name() {
        let (_dir, conn) = db("api-reads-arena-untagged");
        arena_match(&conn, "q-a", "arena:abc-id", Some("Qarena"));
        arena_match(&conn, "q-b", "Qarena#12345", None);
        let inputs = arena_inputs(&conn);
        let untagged = inputs.iter().find(|i| i.match_token == "q-a").expect("a");
        assert_eq!(untagged.username, "Qarena");
        assert_eq!(
            untagged
                .arena
                .as_ref()
                .and_then(|a| a.arena_user_id.as_deref()),
            Some("abc-id")
        );
        let tagged = inputs.iter().find(|i| i.match_token == "q-b").expect("b");
        assert_eq!(tagged.username, "Qarena#12345");
    }

    #[test]
    fn an_account_with_no_name_is_not_reported() {
        let (_dir, conn) = db("api-reads-arena-no-name");
        arena_match(&conn, "q-a", "arena:abc-id", None);
        assert!(report_tokens(&conn).is_empty());
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
    }

    #[test]
    fn the_opponent_name_is_never_a_key_or_too_long() {
        let (_dir, conn) = db("api-reads-arena-opponent");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        let name_of = |display: Option<&str>, username: &str| {
            conn.execute(
                "UPDATE opponents SET username = ?1, display_name = ?2",
                params![username, display],
            )
            .expect("opponent");
            arena_inputs(&conn)[0].opponent_name.clone()
        };
        assert_eq!(name_of(Some("Opp"), "arena:xyz"), Some("Opp".to_string()));
        assert_eq!(name_of(None, "arena:xyz"), None);
        assert_eq!(
            name_of(Some("Opp"), "arena:name:Opp"),
            Some("Opp".to_string())
        );
        assert_eq!(name_of(Some(&"x".repeat(65)), "arena:xyz"), None);
    }

    #[test]
    fn a_drawn_or_unknown_game_is_undecided() {
        let (_dir, conn) = db("api-reads-arena-undecided");
        arena_match(&conn, "q-a", "Qarena#12345", None);
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches WHERE token = 'q-a'", [], |r| {
                r.get(0)
            })
            .expect("id");
        conn.execute(
            "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                 on_play, local_mulligans, opp_mulligans, local_starting_hand_size,
                 turn_count, started_at, ended_at)
             VALUES (?1, 903, 3, 'draw', 'unknown', 1, 0, 0, 7, 9, 1, 2),
                    (?1, 904, 4, 'unknown', 'unknown', 1, 0, 0, 7, 9, 1, 2)",
            [match_id],
        )
        .expect("games");
        let games = report_games(&conn, match_id).expect("games");
        let flags: Vec<(bool, bool)> = games.iter().map(|g| (g.won, g.undecided)).collect();
        assert_eq!(
            flags,
            vec![(true, false), (false, false), (false, true), (false, true)]
        );
    }

    #[test]
    fn oracle_cards_never_offer_an_arena_catalog_id() {
        let (_dir, conn) = db("api-reads-oracle-no-arena");
        let arena_card = cards::ARENA_ID_OFFSET + 70_001;
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (500, 'q-oracle-500'),
                 (900, 'q-oracle-500'), (?1, 'q-oracle-500')",
            [arena_card],
        )
        .expect("cards");
        let deck = [mymtgo_api::report::DeckCard {
            catalog_id: 500,
            quantity: 4,
            sideboard: false,
        }];
        let rows = oracle_cards(&conn, &deck).expect("oracle cards");
        assert_eq!(
            rows,
            vec![
                (500, "q-oracle-500".to_string()),
                (900, "q-oracle-500".to_string())
            ]
        );
    }

    #[test]
    fn the_limit_is_shared_by_both_platforms() {
        let (_dir, conn) = db("api-reads-arena-limit");
        submittable(&conn, "q-m1", &[&[(1001, 1)]]);
        submittable(&conn, "q-m2", &[&[(1001, 1)]]);
        arena_match(&conn, "q-a1", "Qarena#12345", None);
        arena_match(&conn, "q-a2", "Qarena#12345", None);
        let inputs = report_candidates(&conn, 3, VERSION, AFTER_THE_HOLD).expect("candidates");
        let tokens: Vec<&str> = inputs.iter().map(|i| i.match_token.as_str()).collect();
        assert_eq!(tokens, vec!["q-m1", "q-m2", "q-a1"]);
    }

    #[test]
    fn card_stats_skip_arena_matches() {
        let (_dir, conn) = db("api-reads-card-stats-arena");
        mtgo_and_arena_matches(&conn);
        // The Arena game sync's rows: Arena catalog ids with zone data, and
        // the match stamped derived.
        let arena_card = cards::ARENA_ID_OFFSET + 70_001;
        conn.execute(
            "INSERT INTO cards (catalog_id) VALUES (?1) ON CONFLICT DO NOTHING",
            [arena_card],
        )
        .expect("card");
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, seen, has_zone_data)
             SELECT g.id, 'local', ?1, 4, 1, 1 FROM games g
             JOIN matches m ON m.id = g.match_id WHERE m.token = 'q-arena'",
            [arena_card],
        )
        .expect("arena rows");
        conn.execute(
            "INSERT INTO arena_game_syncs (match_id, status, derived_version, synced_at)
             SELECT id, 'derived', 1, 0 FROM matches WHERE token = 'q-arena'",
            [],
        )
        .expect("stamp");
        assert_eq!(
            card_stat_subjects(&conn, 500),
            vec!["q-mtgo:901".to_string()]
        );
    }

    /// `q-mtgo` and `q-endstep`, equally submittable: the Endstep match is
    /// seeded through the write path, then given the seats, deck version
    /// link and game cards `submittable` gives the MTGO one.
    fn mtgo_and_endstep_matches(conn: &mut Connection) {
        submittable(conn, "q-mtgo", &[&[(1001, 1)]]);
        crate::test_support::seed_endstep(conn, "q-endstep");
        conn.execute(
            "UPDATE matches SET source = 'log', ended_at = 1787134600000,
                 last_activity_at = 1787134600000
             WHERE token = 'q-endstep'",
            [],
        )
        .expect("log match");
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, opening_hand,
                 starting_hand, seen, cast, sided_out)
             SELECT g.id, 'local', 1001, 4, 2, 2, 1, 1, 1 FROM games g
             JOIN matches m ON m.id = g.match_id WHERE m.token = 'q-endstep'",
            [],
        )
        .expect("endstep cards");
        seat(conn, "q-endstep", false, PLAYER_UUID, "downloaded");
        seat(conn, "q-endstep", true, OPPONENT_UUID, "downloaded");
    }

    #[test]
    fn reports_never_sends_endstep() {
        let (_dir, mut conn) = db("api-reads-report-endstep");
        mtgo_and_endstep_matches(&mut conn);
        assert_eq!(report_tokens(&conn), vec!["q-mtgo".to_string()]);
    }

    #[test]
    fn pending_reports_never_sends_endstep() {
        let (_dir, mut conn) = db("api-reads-pending-endstep");
        mtgo_and_endstep_matches(&mut conn);
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            1
        );
    }

    #[test]
    fn card_stats_never_sends_endstep() {
        let (_dir, mut conn) = db("api-reads-card-stats-endstep");
        mtgo_and_endstep_matches(&mut conn);
        let subjects = card_stat_subjects(&conn, 500);
        assert!(!subjects.is_empty());
        assert!(
            subjects
                .iter()
                .all(|subject| subject.starts_with("q-mtgo:")),
            "{subjects:?}"
        );
    }

    #[test]
    fn tournament_sync_never_sends_endstep() {
        let (_dir, mut conn) = db("api-reads-tournaments-endstep");
        let before = unqueued_tournament_events(&conn, 500)
            .expect("events")
            .len();
        crate::test_support::seed_endstep(&mut conn, "q-endstep");
        // Endstep writes no log event and no tournament row, so nothing of
        // it is read for the tournament observations or name lookups.
        assert_eq!(
            unqueued_tournament_events(&conn, 500)
                .expect("events")
                .len(),
            before
        );
        assert!(tournaments_to_name(&conn, 50).expect("names").is_empty());
    }

    // 0.x `scopeSubmittable` and the action's guards.
    #[test]
    fn report_candidates_are_complete_linked_log_matches_with_a_player_archetype() {
        let (_dir, mut conn) = db("api-reads-report-eligible");
        for token in [
            "q-ok",
            "q-no-format",
            "q-in-progress",
            "q-no-deck",
            "q-manual",
            "q-queued",
        ] {
            submittable(&conn, token, &[&[(1001, 1)]]);
        }
        conn.execute_batch(
            "UPDATE matches SET format = NULL WHERE token = 'q-no-format';
             UPDATE matches SET state = 'in_progress' WHERE token = 'q-in-progress';
             UPDATE matches SET deck_version_id = NULL WHERE token = 'q-no-deck';
             UPDATE matches SET source = 'manual' WHERE token = 'q-manual';",
        )
        .expect("variants");
        crate::outbox::edit_outbox(
            &mut conn,
            &crate::outbox::OutboxEdit::Enqueue {
                rows: vec![NewOutboxRow {
                    kind: OutboxKind::MatchReport,
                    subject: "q-queued".to_string(),
                    payload: Some(b"q".to_vec()),
                    now_ms: 1,
                }],
            },
        )
        .expect("queued");
        assert_eq!(
            report_tokens(&conn),
            vec!["q-ok".to_string(), "q-no-format".to_string()]
        );
        assert_eq!(
            report_candidates(&conn, 1, VERSION, AFTER_THE_HOLD)
                .expect("candidates")
                .len(),
            1
        );
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            3,
            "two to queue and one queued"
        );
        // No player archetype: nothing to queue.
        for token in ["q-ok", "q-no-format"] {
            clear_seat(&conn, token, false);
        }
        assert!(
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD)
                .expect("candidates")
                .is_empty()
        );
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            1
        );
    }

    // Ruling 4: `league_run` is the stored `matches.league_id`, and the
    // league token is its row's.
    #[test]
    fn league_run_is_the_stored_league_id() {
        let (_dir, conn) = db("api-reads-report-league");
        submittable(&conn, "q-league-match", &[&[(1001, 1)]]);
        conn.execute(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (42, 'q-league:q-league-match', 1, 'log', 'constructed', 'complete',
                 'q-league-token', 'Qleague', 0, 0)",
            [],
        )
        .expect("league");
        conn.execute(
            "UPDATE matches SET league_id = 42 WHERE token = 'q-league-match'",
            [],
        )
        .expect("member");
        let input = &report_candidates(&conn, 10, VERSION, AFTER_THE_HOLD).expect("candidates")[0];
        assert_eq!(input.league_id, Some(42));
        let body = report_body(input);
        assert_eq!(body["league_run"], serde_json::json!(42));
        assert_eq!(body["league_token"], serde_json::json!("q-league-token"));
        // League run identity: the league's sync client id, as the match
        // bundle carries it.
        assert_eq!(
            input.league_client_id.as_deref(),
            Some("q-league-token_19700101T000000Z")
        );
        assert_eq!(
            body["league_client_id"],
            serde_json::json!("q-league-token_19700101T000000Z")
        );
    }

    // MTGO's league offering id: the stored `leagues.event_id` (the log's
    // `EventId`), for a league from MTGO; a manual league and a match with
    // no league send none.
    #[test]
    fn a_league_match_reports_its_playevent_id() {
        let (_dir, conn) = db("api-reads-report-playevent");
        submittable(&conn, "q-log-league", &[&[(1001, 1)]]);
        submittable(&conn, "q-manual-league", &[&[(1001, 1)]]);
        submittable(&conn, "q-no-league", &[&[(1001, 1)]]);
        conn.execute_batch(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, event_id,
                 name, started_at, created_at)
             VALUES (44, 'q-league:q-log-league', 1, 'log', 'constructed', 'complete',
                 'q-league-token', 10397, 'Qleague', 0, 0),
                 (45, 'manual:45', 1, 'manual', 'constructed', 'complete',
                 'manual_qqqqqqqqqqqqqqqqqqqqqqqq', 10397, 'Qmanual', 0, 0);
             UPDATE matches SET league_id = 44 WHERE token = 'q-log-league';
             UPDATE matches SET league_id = 45 WHERE token = 'q-manual-league';",
        )
        .expect("leagues");
        let inputs = report_candidates(&conn, 10, VERSION, AFTER_THE_HOLD).expect("candidates");
        assert_eq!(inputs.len(), 3);
        for input in &inputs {
            let expected = match input.match_token.as_str() {
                "q-log-league" => Some(10397),
                _ => None,
            };
            assert_eq!(input.league_playevent_id, expected, "{}", input.match_token);
        }
    }

    // The bundle's null rule: a league with no token sends no client id,
    // even when the row holds one; a match with no league sends none.
    #[test]
    fn a_league_with_no_token_sends_no_client_id() {
        let (_dir, conn) = db("api-reads-report-no-token");
        submittable(&conn, "q-tokenless", &[&[(1001, 1)]]);
        submittable(&conn, "q-leagueless", &[&[(1001, 1)]]);
        conn.execute(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at, imported_from, imported_seed, sync_client_id)
             VALUES (43, '0x:7', 1, 'imported', 'constructed', 'complete',
                 NULL, 'Qleague', 0, 0, 7, '{}', 'q-stale_19700101T000000Z')",
            [],
        )
        .expect("league");
        conn.execute(
            "UPDATE matches SET league_id = 43 WHERE token = 'q-tokenless'",
            [],
        )
        .expect("member");
        let inputs = report_candidates(&conn, 10, VERSION, AFTER_THE_HOLD).expect("candidates");
        assert_eq!(inputs.len(), 2);
        for input in &inputs {
            assert_eq!(input.league_client_id, None, "{}", input.match_token);
            assert!(report_body(input).get("league_client_id").is_none());
        }
    }

    // The report body from stored rows (0.x SubmitMatchToApiTest's fields).
    #[test]
    fn a_report_is_built_from_the_stored_match() {
        let (_dir, conn) = db("api-reads-report-input");
        submittable(&conn, "q-match", &[&[(2001, 3)], &[(2001, 3), (2002, 1)]]);
        conn.execute_batch(
            "UPDATE accounts SET login_id = 6031972 WHERE username = 'Qplayer';
             UPDATE matches SET tournament_event_id = 77, tournament_round = 2,
                 tournament_token = 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb';
             INSERT INTO cards (catalog_id, oracle_id) VALUES (500, 'q-oracle-a'),
                 (501, 'q-oracle-a'), (600, 'q-oracle-b');",
        )
        .expect("details");
        let inputs = report_candidates(&conn, 10, VERSION, AFTER_THE_HOLD).expect("candidates");
        let body = report_body(&inputs[0]);
        let expected = serde_json::json!({
            "match_token": "q-match",
            "username": "Qplayer",
            "login_id": 6031972,
            "player_archetype_uuid": PLAYER_UUID,
            "opponent_archetype_uuid": OPPONENT_UUID,
            "result": "win",
            "format": "CMODERN",
            "is_tournament": true,
            "league_token": null,
            "league_run": null,
            "challenge_token": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
            "tournament_round": 2,
            "played_at": "2026-08-19T10:00:00+00:00",
            "client_version": VERSION,
            "opponent_name": "Qfoe",
            "deck": [
                {"mtgo_id": 501, "quantity": 4, "zone": "main"},
                {"mtgo_id": 600, "quantity": 2, "zone": "side"},
            ],
            "opponent_deck": [
                {"mtgo_id": 2001, "quantity": 4, "zone": "main"},
                {"mtgo_id": 2002, "quantity": 1, "zone": "main"},
            ],
            "games": [
                {
                    "game_number": 1, "turn_count": 9,
                    "started_at": "2026-08-19T10:01:00+00:00",
                    "ended_at": "2026-08-19T10:01:30+00:00",
                    "mulligan_count": 0, "starting_hand_size": 3,
                    "kept_hand": [1001, 1001, 1002],
                    "opponent_mulligan_count": 1, "on_play": true, "won": true,
                    "clock_remaining_ms": null, "opponent_clock_remaining_ms": null,
                    "timed_out": null, "opponent_timed_out": null,
                },
                {
                    "game_number": 2, "turn_count": 9,
                    "started_at": "2026-08-19T10:02:00+00:00",
                    "ended_at": "2026-08-19T10:02:30+00:00",
                    "mulligan_count": 0, "starting_hand_size": 3,
                    "kept_hand": [1001, 1001, 1002],
                    "opponent_mulligan_count": 1, "on_play": true, "won": true,
                    "clock_remaining_ms": null, "opponent_clock_remaining_ms": null,
                    "timed_out": null, "opponent_timed_out": null,
                },
            ],
        });
        assert_eq!(body, expected);
    }

    fn card_stat_subjects(conn: &Connection, limit: u32) -> Vec<String> {
        card_stats_candidates(conn, limit, AFTER_THE_HOLD)
            .expect("candidates")
            .into_iter()
            .map(|game| format!("{}:{}", game.match_token, game.mtgo_game_id))
            .collect()
    }

    // 0.x EnqueueCardStatsTest: "enqueues an eligible game exactly once",
    // "is idempotent on re-run", the skips, and "respects limit parameter".
    #[test]
    fn card_stat_candidates_follow_the_0x_rules() {
        let (_dir, mut conn) = db("api-reads-card-stats");
        submittable(&conn, "q-ok", &[&[(1001, 1)], &[]]);
        for token in ["q-open", "q-no-deck", "q-manual"] {
            submittable(&conn, token, &[&[(1001, 1)]]);
        }
        submittable(&conn, "q-undecided", &[&[(1001, 1)]]);
        conn.execute_batch(
            "UPDATE matches SET state = 'in_progress' WHERE token = 'q-open';
             UPDATE matches SET deck_version_id = NULL WHERE token = 'q-no-deck';
             UPDATE matches SET source = 'manual' WHERE token = 'q-manual';
             UPDATE games SET result = 'unknown'
                 WHERE match_id = (SELECT id FROM matches WHERE token = 'q-undecided');
             UPDATE games SET result = 'loss' WHERE game_number = 2;",
        )
        .expect("variants");
        // A game whose cards are all the opponent's.
        conn.execute(
            "DELETE FROM game_cards WHERE side = 'local' AND game_id = (SELECT g.id FROM games g
                 JOIN matches m ON m.id = g.match_id WHERE m.token = 'q-ok' AND g.game_number = 2)",
            [],
        )
        .expect("opponent only");
        assert_eq!(card_stat_subjects(&conn, 500), vec!["q-ok:901".to_string()]);
        // Queued once: never read again.
        crate::outbox::edit_outbox(
            &mut conn,
            &crate::outbox::OutboxEdit::Enqueue {
                rows: vec![NewOutboxRow {
                    kind: OutboxKind::CardStats,
                    subject: "q-ok:901".to_string(),
                    payload: Some(b"q".to_vec()),
                    now_ms: 1,
                }],
            },
        )
        .expect("queued");
        assert!(card_stat_subjects(&conn, 500).is_empty());
        // The limit.
        submittable(&conn, "q-more", &[&[], &[]]);
        conn.execute("UPDATE games SET result = 'win'", [])
            .expect("all decided");
        assert_eq!(card_stat_subjects(&conn, 2).len(), 2);
    }

    /// When the `submittable` fixture's matches end.
    const FIXTURE_ENDED_AT: i64 = 1_787_134_600_000;

    // Edit a match spec section 5: a match waits 5 minutes after it ends,
    // so a fix made right after it goes out in the first send.
    #[test]
    fn a_match_is_reported_only_5_minutes_after_it_ended() {
        let (_dir, conn) = db("api-reads-report-hold");
        submittable(&conn, "q-held", &[&[(1001, 1)]]);
        let four_minutes = FIXTURE_ENDED_AT + 4 * 60_000;
        let five_minutes = FIXTURE_ENDED_AT + SEND_HOLD_MS;
        let tokens = |now_ms: i64| -> Vec<String> {
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, now_ms)
                .expect("candidates")
                .into_iter()
                .map(|input| input.match_token)
                .collect()
        };
        assert!(tokens(four_minutes).is_empty());
        assert_eq!(pending_report_count(&conn, four_minutes).expect("count"), 0);
        assert_eq!(tokens(five_minutes), vec!["q-held".to_string()]);
        assert_eq!(pending_report_count(&conn, five_minutes).expect("count"), 1);
        // A match with no end waits on its last activity.
        conn.execute(
            "UPDATE matches SET ended_at = NULL, last_activity_at = ?1",
            [FIXTURE_ENDED_AT + 60_000],
        )
        .expect("no end");
        assert!(tokens(five_minutes).is_empty());
        assert_eq!(tokens(five_minutes + 60_000), vec!["q-held".to_string()]);
    }

    #[test]
    fn card_stats_wait_5_minutes_after_the_match_ended() {
        let (_dir, conn) = db("api-reads-card-stats-hold");
        submittable(&conn, "q-held", &[&[(1001, 1)]]);
        let subjects = |now_ms: i64| -> Vec<String> {
            card_stats_candidates(&conn, ENQUEUE_LIMIT, now_ms)
                .expect("candidates")
                .into_iter()
                .map(|game| format!("{}:{}", game.match_token, game.mtgo_game_id))
                .collect()
        };
        assert!(subjects(FIXTURE_ENDED_AT + 4 * 60_000).is_empty());
        assert_eq!(
            subjects(FIXTURE_ENDED_AT + SEND_HOLD_MS),
            vec!["q-held:901".to_string()]
        );
    }

    // 0.x "stores serialized payload at enqueue time": the stored columns.
    #[test]
    fn a_card_stats_game_is_read_from_the_local_side() {
        let (_dir, conn) = db("api-reads-card-stats-input");
        submittable(&conn, "q-match", &[&[(2001, 1)]]);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1');
             UPDATE game_cards SET cast_turn = 2, played = 1, activated = NULL
                 WHERE catalog_id = 1001;
             INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
                 SELECT game_id, 'local', 1001, 'kicked', 2 FROM game_cards
                 WHERE catalog_id = 1001;",
        )
        .expect("card details");
        let games = card_stats_candidates(&conn, 10, AFTER_THE_HOLD).expect("read");
        assert_eq!(games.len(), 1);
        let game = &games[0];
        assert_eq!(game.match_token, "q-match");
        assert_eq!(game.mtgo_game_id, 901);
        assert_eq!(game.local_player.as_deref(), Some("Qplayer"));
        assert_eq!(game.player_archetype_uuid.as_deref(), Some(PLAYER_UUID));
        assert_eq!(game.opponent_archetype_uuid.as_deref(), Some(OPPONENT_UUID));
        assert_eq!(game.format.as_deref(), Some("CMODERN"));
        assert!(game.won);
        assert_eq!(game.on_play, Some(true));
        assert_eq!(game.game_number, 1);
        assert_eq!(game.game_started_at_ms, Some(1_787_133_660_000));
        assert_eq!(game.match_started_at_ms, Some(1_787_133_600_000));
        assert_eq!(game.cards.len(), 2, "the local side only");
        let card = &game.cards[0];
        assert_eq!(card.oracle_id.as_deref(), Some("q-oracle-1"));
        assert_eq!(
            (card.quantity, card.kept, card.seen, card.cast),
            (4, 2, 1, 1)
        );
        assert_eq!(
            (card.cast_turn, card.played, card.activated),
            (Some(2), Some(1), None)
        );
        assert_eq!(card.casts, vec![("kicked".to_string(), 2)]);
        assert_eq!((card.sided_in, card.sided_out), (0, 1));
        assert_eq!(game.cards[1].oracle_id, None, "1002 has no card row");
        let payload = mymtgo_api::card_stats_payload(game).expect("payload");
        assert_eq!(payload["cards"].as_array().expect("cards").len(), 1);
        assert_eq!(payload["played_on"], serde_json::json!("2026-08-19"));
    }

    // Cards spec section 11, readiness 4 (Plan Q task 12): a game 0.x queued
    // card stats for keeps its imported row and state, whatever the state,
    // and is never read for queueing again; a game 0.x never queued is.
    #[test]
    fn nothing_0x_shipped_is_enqueued_again() {
        use crate::import_api::{ImportedApiState, LegacyCardStats, import_api_state};
        let (_dir, mut conn) = db("api-reads-card-stats-imported");
        submittable(&conn, "q-imported", &[&[], &[], &[], &[], &[]]);
        let legacy = |game: i64, status: &str| LegacyCardStats {
            match_token: "q-imported".to_string(),
            mtgo_match_id: None,
            game_mtgo_id: Some(900 + game),
            payload: format!(r#"{{"game":{game}}}"#),
            status: status.to_string(),
            attempts: 1,
            next_attempt_at_ms: None,
            last_error: None,
            created_at_ms: Some(10),
            updated_at_ms: Some(20),
        };
        let state = ImportedApiState {
            card_stats: vec![
                legacy(1, "sent"),
                legacy(2, "failed"),
                legacy(3, "skipped"),
                legacy(4, "sending"),
            ],
            ..ImportedApiState::default()
        };
        let states = |conn: &Connection| -> Vec<String> {
            let mut stmt = conn
                .prepare(
                    "SELECT subject || ' ' || status FROM outbox
                     WHERE kind = 'card_stats' ORDER BY subject",
                )
                .expect("prepare");
            stmt.query_map([], |row| row.get(0))
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        let imported = import_api_state(&mut conn, &state, 1_000).expect("import");
        assert_eq!(imported.card_stats_added, 4);
        let before = states(&conn);
        assert_eq!(
            before,
            vec![
                "q-imported:901 sent".to_string(),
                "q-imported:902 failed".to_string(),
                "q-imported:903 skipped".to_string(),
                "q-imported:904 pending".to_string(),
            ]
        );
        assert_eq!(
            card_stat_subjects(&conn, 500),
            vec!["q-imported:905".to_string()],
            "only the game 0.x never queued"
        );
        // A second import changes nothing either.
        import_api_state(&mut conn, &state, 2_000).expect("again");
        assert_eq!(states(&conn), before);
        assert_eq!(
            card_stat_subjects(&conn, 500),
            vec!["q-imported:905".to_string()]
        );
    }

    // ---- The archetype check in SQL (archetypes spec section 13) ----------

    // Q8's closure cases rerun on stored rows: both seats, the player seat
    // only, the opponent seat only, none. A report needs the player seat;
    // so do card stats since F23 (0.x read a game with either seat and then
    // skipped it without the player's), leaving a missing opponent uuid out.
    #[test]
    fn the_sql_archetype_check_matches_the_old_rules() {
        let (_dir, conn) = db("api-reads-sql-check");
        for token in ["q-both", "q-player", "q-opponent", "q-none"] {
            submittable(&conn, token, &[&[(1001, 1)]]);
        }
        clear_seat(&conn, "q-player", true);
        clear_seat(&conn, "q-opponent", false);
        clear_seat(&conn, "q-none", false);
        clear_seat(&conn, "q-none", true);

        let reports =
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD).expect("candidates");
        let seen: Vec<(&str, Option<&str>, Option<&str>)> = reports
            .iter()
            .map(|input| {
                (
                    input.match_token.as_str(),
                    input
                        .player_archetype
                        .as_ref()
                        .map(|side| side.uuid.as_str()),
                    input
                        .opponent_archetype
                        .as_ref()
                        .map(|side| side.uuid.as_str()),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("q-both", Some(PLAYER_UUID), Some(OPPONENT_UUID)),
                ("q-player", Some(PLAYER_UUID), None),
            ]
        );
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            2
        );
        let first = report_candidates(&conn, 1, VERSION, AFTER_THE_HOLD).expect("candidates");
        assert_eq!(first.len(), 1, "the limit is the SQL's");
        assert_eq!(first[0].match_token, "q-both");

        let games = card_stats_candidates(&conn, 500, AFTER_THE_HOLD).expect("read");
        let seen: Vec<(&str, Option<&str>, Option<&str>)> = games
            .iter()
            .map(|game| {
                (
                    game.match_token.as_str(),
                    game.player_archetype_uuid.as_deref(),
                    game.opponent_archetype_uuid.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("q-both", Some(PLAYER_UUID), Some(OPPONENT_UUID)),
                ("q-player", Some(PLAYER_UUID), None),
            ]
        );
        let first = card_stats_candidates(&conn, 1, AFTER_THE_HOLD).expect("read");
        assert_eq!(first.len(), 1, "the limit is the SQL's");
        assert_eq!(first[0].match_token, "q-both");
    }

    // 0.x SubmitMatchToApi: `if (! $playerArchetype?->archetype) return;`.
    // A match with only the opponent's seat, even a hand-set one, is never
    // reported or counted, and its card stats are never read (F23: 0.x read
    // them and skipped them). Once the player seat is stored both go.
    #[test]
    fn nothing_is_reported_without_a_player_seat() {
        let (_dir, conn) = db("api-reads-no-player-seat");
        submittable(&conn, "q-match", &[&[(1001, 1)]]);
        clear_seat(&conn, "q-match", false);
        conn.execute("UPDATE match_archetypes SET manual = 1", [])
            .expect("hand-set opponent");
        assert!(
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD)
                .expect("candidates")
                .is_empty()
        );
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            0
        );
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1')",
            [],
        )
        .expect("card");
        assert!(
            card_stats_candidates(&conn, 500, AFTER_THE_HOLD)
                .expect("read")
                .is_empty()
        );

        seat(&conn, "q-match", false, PLAYER_UUID, "downloaded");
        let reports =
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD).expect("candidates");
        assert_eq!(reports.len(), 1);
        assert_eq!(
            pending_report_count(&conn, AFTER_THE_HOLD).expect("count"),
            1
        );
        let games = card_stats_candidates(&conn, 500, AFTER_THE_HOLD).expect("read");
        assert_eq!(games.len(), 1);
        assert!(mymtgo_api::card_stats_payload(&games[0]).is_some());
    }

    // 0.x SubmitMatchToApiTest "sends a null opponent archetype uuid when the
    // opponent archetype is local to this client", "sends a null player
    // archetype uuid when the player archetype is local to this client" and
    // "sends a null opponent archetype uuid for the fallback archetypes":
    // `ArchetypeRef` reads `manual` and `is_fallback` from `archetypes.source`.
    #[test]
    fn manual_and_fallback_archetypes_go_as_null_in_reports() {
        let (_dir, conn) = db("api-reads-report-local-uuids");
        submittable(&conn, "q-local-opponent", &[&[(3001, 1)]]);
        seat(&conn, "q-local-opponent", true, MANUAL_UUID, "manual");
        submittable(&conn, "q-local-player", &[&[(3002, 1)]]);
        seat(&conn, "q-local-player", false, MANUAL_UUID, "manual");
        submittable(&conn, "q-homebrew", &[&[(3003, 1)]]);
        seat(
            &conn,
            "q-homebrew",
            true,
            mymtgo_api::HOMEBREW_UUID,
            "fallback",
        );
        let inputs =
            report_candidates(&conn, ENQUEUE_LIMIT, VERSION, AFTER_THE_HOLD).expect("candidates");
        assert_eq!(inputs.len(), 3, "manual and fallback seats are reported");
        let by_token = |token: &str| {
            inputs
                .iter()
                .find(|input| input.match_token == token)
                .unwrap_or_else(|| panic!("{token}"))
        };

        let local_opponent = by_token("q-local-opponent");
        let side = local_opponent.opponent_archetype.as_ref().expect("seat");
        assert_eq!(
            (side.uuid.as_str(), side.manual, side.is_fallback),
            (MANUAL_UUID, true, false)
        );
        let body = report_body(local_opponent);
        assert_eq!(body["opponent_archetype_uuid"], serde_json::Value::Null);
        assert_eq!(body["player_archetype_uuid"], PLAYER_UUID);

        let local_player = by_token("q-local-player");
        let side = local_player.player_archetype.as_ref().expect("seat");
        assert_eq!((side.manual, side.is_fallback), (true, false));
        let body = report_body(local_player);
        assert_eq!(body["player_archetype_uuid"], serde_json::Value::Null);
        assert_eq!(body["opponent_archetype_uuid"], OPPONENT_UUID);

        let homebrew = by_token("q-homebrew");
        let side = homebrew.opponent_archetype.as_ref().expect("seat");
        assert_eq!(
            (side.uuid.as_str(), side.manual, side.is_fallback),
            (mymtgo_api::HOMEBREW_UUID, false, true)
        );
        let player = homebrew.player_archetype.as_ref().expect("seat");
        assert_eq!((player.manual, player.is_fallback), (false, false));
        assert_eq!(
            report_body(homebrew)["opponent_archetype_uuid"],
            serde_json::Value::Null
        );
    }

    // Archetypes spec section 13: the card stats payload keeps Q8's rules,
    // so manual and fallback uuids are sent as stored (0.x
    // `BuildCardStatsGamePayload` does not null them).
    #[test]
    fn card_stats_keep_manual_and_fallback_uuids() {
        let (_dir, conn) = db("api-reads-card-stats-local-uuids");
        submittable(&conn, "q-match", &[&[(1001, 1)]]);
        seat(&conn, "q-match", false, MANUAL_UUID, "manual");
        seat(&conn, "q-match", true, mymtgo_api::ROGUE_UUID, "fallback");
        let games = card_stats_candidates(&conn, 500, AFTER_THE_HOLD).expect("read");
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].player_archetype_uuid.as_deref(), Some(MANUAL_UUID));
        assert_eq!(
            games[0].opponent_archetype_uuid.as_deref(),
            Some(mymtgo_api::ROGUE_UUID)
        );
    }

    /// Add a local `game_cards` row with an oracle id (`q-oracle-{id}`) to
    /// game `number` of the match `token`; `columns` and `values` name the
    /// extra columns set.
    fn local_row(conn: &Connection, token: &str, number: i64, catalog_id: u32, columns: &str) {
        let (names, values) = if columns.is_empty() {
            (String::new(), String::new())
        } else {
            let mut names = String::new();
            let mut values = String::new();
            for pair in columns.split(',') {
                let (name, value) = pair.split_once('=').expect("name=value");
                names.push_str(", ");
                names.push_str(name.trim());
                values.push_str(", ");
                values.push_str(value.trim());
            }
            (names, values)
        };
        conn.execute(
            &format!(
                "INSERT INTO game_cards (game_id, side, catalog_id{names})
                 VALUES ((SELECT g.id FROM games g JOIN matches m ON m.id = g.match_id
                          WHERE m.token = ?1 AND g.game_number = ?2), 'local', ?3{values})"
            ),
            params![token, number, catalog_id],
        )
        .expect("local row");
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (?1, 'q-oracle-' || ?1)
             ON CONFLICT (catalog_id) DO NOTHING",
            [catalog_id],
        )
        .expect("card");
    }

    /// The oracle ids each read game would ship, as `"{subject} {oracle}"`.
    fn shipped_oracles(conn: &Connection) -> Vec<String> {
        let mut shipped = Vec::new();
        for game in card_stats_candidates(conn, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("read") {
            let payload = mymtgo_api::card_stats_payload(&game).expect("payload");
            for card in payload["cards"].as_array().expect("cards") {
                shipped.push(format!(
                    "{}:{} {}",
                    game.match_token,
                    game.mtgo_game_id,
                    card["oracle_id"].as_str().expect("oracle id")
                ));
            }
        }
        shipped
    }

    // Issue #295: the shipping read sends only the rows 0.x's
    // `card_game_stats` would have had (`card_stats::deck_row`, the card
    // stats tab's predicate): a game 1 sideboard row and a token or created
    // card are left out; a postboard sideboard row, a sided flag and
    // (non-log matches only) 0.x's all-zero postboard row still go.
    #[test]
    fn card_stats_ship_only_the_rows_0x_had() {
        let (_dir, conn) = db("api-reads-card-stats-deck-rows");
        submittable(&conn, "q-log", &[&[], &[]]);
        submittable(&conn, "q-imported", &[&[], &[]]);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1001');
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1002, 'q-oracle-1002');
             UPDATE matches SET source = 'imported' WHERE token = 'q-imported';",
        )
        .expect("cards");
        // Game 1: a sideboard card and a token, neither shipped.
        local_row(&conn, "q-log", 1, 3001, "side_qty = 2");
        local_row(&conn, "q-log", 1, 3002, "seen = 1, cast = 1");
        // Game 2: the sideboard card and a sided flag ship; the token, an
        // inferred row with no deck quantity (S8-7) and a log match's
        // all-zero row do not.
        local_row(&conn, "q-log", 2, 3001, "side_qty = 2");
        local_row(&conn, "q-log", 2, 3002, "seen = 1, cast = 1");
        local_row(&conn, "q-log", 2, 3003, "sided_out = 1");
        local_row(&conn, "q-log", 2, 3004, "inferred = 1, seen = 1");
        local_row(&conn, "q-log", 2, 3005, "");
        // A non-log match: 0.x's all-zero row ships postboard, not in game 1.
        local_row(&conn, "q-imported", 1, 3005, "");
        local_row(&conn, "q-imported", 2, 3005, "");

        let deck = |subject: &str| {
            vec![
                format!("{subject} q-oracle-1001"),
                format!("{subject} q-oracle-1002"),
            ]
        };
        let mut expected = deck("q-log:901");
        expected.extend(deck("q-log:902"));
        expected.extend([
            "q-log:902 q-oracle-3001".to_string(),
            "q-log:902 q-oracle-3003".to_string(),
        ]);
        expected.extend(deck("q-imported:901"));
        expected.extend(deck("q-imported:902"));
        expected.push("q-imported:902 q-oracle-3005".to_string());
        assert_eq!(shipped_oracles(&conn), expected);
    }

    // Review I1: an imported game's casts logged under a variant's catalog
    // id (warp) ship on the deck card's oracle, as 0.x shipped them: the
    // imported rule folds the variant row into the registered printing. The
    // rows are written directly, as a history import has them (no 0.x card
    // stats).
    #[test]
    fn an_imported_games_variant_casts_ship_on_the_deck_card() {
        let (_dir, conn) = db("api-reads-card-stats-variant");
        submittable(&conn, "q-imported", &[&[], &[]]);
        let registered = decks::signature(&[
            mymtgo_api::report::DeckCard {
                catalog_id: 1001,
                quantity: 4,
                sideboard: false,
            },
            mymtgo_api::report::DeckCard {
                catalog_id: 1002,
                quantity: 4,
                sideboard: false,
            },
        ]);
        conn.execute("UPDATE deck_versions SET signature = ?1", [&registered])
            .expect("signature");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1001');
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1002, 'q-oracle-1002');
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1901, 'q-oracle-1001');
             UPDATE matches SET source = 'imported' WHERE token = 'q-imported';",
        )
        .expect("cards");
        local_row(&conn, "q-imported", 2, 1901, "seen = 1, cast = 2");
        let match_id: i64 = conn
            .query_row(
                "SELECT id FROM matches WHERE token = 'q-imported'",
                [],
                |row| row.get(0),
            )
            .expect("match");
        let entry = |at: i64, message: &str| game_detail::GameLogEntry {
            timestamp_ms: at,
            message: message.to_string(),
        };
        let log = vec![
            entry(1_000, "@P@PQplayer joined the game."),
            entry(1_001, "@P@PQfoe joined the game."),
            entry(1_002, "@PQplayer wins the game."),
            entry(2_000, "@P@PQplayer joined the game."),
            entry(2_001, "@P@PQfoe joined the game."),
            entry(2_002, "@PTurn 1: Qplayer"),
            entry(2_003, "@PQplayer casts @[Qcard@:3802,201:@]."),
            entry(2_004, "@PQplayer wins the game."),
        ];
        let tx = conn.unchecked_transaction().expect("tx");
        crate::game_sync::store_game_log(&tx, match_id, &log).expect("log");
        crate::imported_detail::reapply_imported_rule(&tx, match_id).expect("rule");
        tx.commit().expect("commit");

        let mut casts = Vec::new();
        for game in card_stats_candidates(&conn, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("read") {
            let payload = mymtgo_api::card_stats_payload(&game).expect("payload");
            for card in payload["cards"].as_array().expect("cards") {
                if card["oracle_id"] == "q-oracle-1001" {
                    casts.push((game.mtgo_game_id, card["cast"].as_i64().expect("cast")));
                }
            }
        }
        assert_eq!(casts, vec![(901, 1), (902, 3)]);
    }

    // Issue #319: a log match's casts logged under a variant's catalog id
    // (warp) ship on the deck card's oracle, as 0.x's widened catalog to
    // oracle map shipped them: one row, the casts added, the copies seen
    // and the zone counts taken from the deck card (the same instances),
    // and no discard for a card that was cast. A variant whose oracle is not
    // a deck card that game still does not ship.
    #[test]
    fn a_log_matchs_variant_casts_ship_on_the_deck_card() {
        let (_dir, conn) = db("api-reads-card-stats-log-variant");
        submittable(&conn, "q-log", &[&[], &[]]);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1001');
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1002, 'q-oracle-1002');
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1901, 'q-oracle-1001');
             UPDATE game_cards SET hand_seen = 1, discarded = 1, has_zone_data = 1
                 WHERE catalog_id = 1001;
             UPDATE game_cards SET cast = 0
                 WHERE catalog_id = 1001
                   AND game_id = (SELECT id FROM games WHERE mtgo_game_id = 901);",
        )
        .expect("cards");
        local_row(
            &conn,
            "q-log",
            1,
            1901,
            "seen = 1, cast = 2, hand_seen = 1, battlefield_seen = 1, has_zone_data = 1",
        );
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES ((SELECT id FROM games WHERE mtgo_game_id = 901), 'local', 1901, 'warp', 2)",
            [],
        )
        .expect("casts");
        // A variant of a card not in the deck is a card of its own.
        local_row(&conn, "q-log", 2, 1902, "seen = 1, cast = 1");

        let mut shipped = Vec::new();
        for game in card_stats_candidates(&conn, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("read") {
            let payload = mymtgo_api::card_stats_payload(&game).expect("payload");
            for card in payload["cards"].as_array().expect("cards") {
                if card["oracle_id"] == "q-oracle-1001" {
                    shipped.push((
                        game.mtgo_game_id,
                        card["quantity"].as_i64().expect("quantity"),
                        card["cast"].as_i64().expect("cast"),
                        card["warp"].as_i64().expect("warp"),
                        card["seen"].as_i64().expect("seen"),
                        card["hand_seen"].as_i64().expect("hand"),
                        card["battlefield_seen"].as_i64().expect("battlefield"),
                        card["discarded"].as_i64().expect("discarded"),
                    ));
                }
            }
        }
        assert_eq!(
            shipped,
            vec![(901, 4, 2, 2, 1, 1, 1, 0), (902, 4, 1, 0, 1, 1, 0, 1)]
        );
        let oracles = shipped_oracles(&conn);
        assert!(!oracles.iter().any(|o| o.ends_with("q-oracle-1902")));
        assert_eq!(oracles.len(), 4, "{oracles:?}");
    }

    /// The shipped row of `oracle` per game: `(game, quantity, kept, seen,
    /// cast, warp)`.
    fn shipped_counts(conn: &Connection, oracle: &str) -> Vec<(i64, i64, i64, i64, i64, i64)> {
        let mut shipped = Vec::new();
        for game in card_stats_candidates(conn, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("read") {
            let payload = mymtgo_api::card_stats_payload(&game).expect("payload");
            for card in payload["cards"].as_array().expect("cards") {
                if card["oracle_id"] == oracle {
                    shipped.push((
                        game.mtgo_game_id,
                        card["quantity"].as_i64().expect("quantity"),
                        card["kept"].as_i64().expect("kept"),
                        card["seen"].as_i64().expect("seen"),
                        card["cast"].as_i64().expect("cast"),
                        card["warp"].as_i64().expect("warp"),
                    ));
                }
            }
        }
        shipped
    }

    // Issue #320: a multi-face card's face logged under its own catalog id
    // (a stub with the log's face name, no oracle id) ships on the deck
    // card whose face it is, as 0.x's `resolveFaceNamesToOracle` mapped it.
    #[test]
    fn a_face_logged_under_its_own_id_ships_on_the_deck_card() {
        let (_dir, conn) = db("api-reads-card-stats-face");
        submittable(&conn, "q-log", &[&[], &[]]);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name)
                 VALUES (1001, 'q-oracle-1001', 'Qmain // Qface');
             INSERT INTO cards (catalog_id, oracle_id) VALUES (1002, 'q-oracle-1002');
             INSERT INTO cards (catalog_id, name) VALUES (1951, 'Qface'), (1952, 'Qnothing');",
        )
        .expect("cards");
        local_row(&conn, "q-log", 1, 1951, "seen = 1, cast = 2");
        local_row(&conn, "q-log", 1, 1952, "seen = 1, cast = 2");
        conn.execute(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES ((SELECT id FROM games WHERE mtgo_game_id = 901), 'local', 1951, 'warp', 2)",
            [],
        )
        .expect("casts");

        assert_eq!(
            shipped_counts(&conn, "q-oracle-1001"),
            vec![(901, 4, 2, 1, 3, 2), (902, 4, 2, 1, 1, 0)]
        );
        assert_eq!(shipped_oracles(&conn).len(), 4, "1952 resolves to nothing");
    }

    // Issue #320 (the #319 review's parked cap): with two deck printings of
    // one oracle, a folded variant caps the shipped row at the oracle's
    // total copies, as the card stats tab does, not at the first printing's.
    #[test]
    fn a_folded_variant_caps_at_the_oracle_total() {
        let (_dir, conn) = db("api-reads-card-stats-variant-cap");
        submittable(&conn, "q-log", &[&[]]);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1001'),
                 (1002, 'q-oracle-1002'), (1003, 'q-oracle-1001'), (1901, 'q-oracle-1001');",
        )
        .expect("cards");
        local_row(&conn, "q-log", 1, 1003, "main_qty = 2");
        local_row(
            &conn,
            "q-log",
            1,
            1901,
            "opening_hand = 3, starting_hand = 3, seen = 6, cast = 1",
        );

        // Quantity 4 + 2; kept 2 + 0 + 3; seen the larger of 1 + 0 and 6.
        assert_eq!(
            shipped_counts(&conn, "q-oracle-1001"),
            vec![(901, 6, 5, 6, 2, 0)]
        );
    }

    // The #320 review (F5): a same-oracle row before the target (a sided
    // out printing with a lower catalog id) is merged too, so the payload
    // never adds it after the cap.
    #[test]
    fn a_folded_variant_caps_with_an_earlier_printing_merged() {
        let (_dir, conn) = db("api-reads-card-stats-variant-earlier");
        submittable(&conn, "q-log", &[&[], &[]]);
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (999, 'q-oracle-1001'),
                 (1001, 'q-oracle-1001'), (1002, 'q-oracle-1002'), (1901, 'q-oracle-1001');",
        )
        .expect("cards");
        local_row(
            &conn,
            "q-log",
            2,
            999,
            "sided_out = 1, opening_hand = 1, starting_hand = 1, seen = 3",
        );
        local_row(&conn, "q-log", 2, 1901, "seen = 4");

        let game_two: Vec<_> = shipped_counts(&conn, "q-oracle-1001")
            .into_iter()
            .filter(|row| row.0 == 902)
            .collect();
        // Kept 2 + 1; seen the larger of 1 + 3 and 4, at most 4.
        assert_eq!(game_two, vec![(902, 4, 3, 4, 1, 0)]);
    }

    // Plan T follow-up F23: a game without a player archetype is never
    // shipped (0.x), and is now left out in SQL, so 600 of them cannot fill
    // the 500-game batch ahead of an eligible game.
    #[test]
    fn games_without_a_player_archetype_never_fill_the_batch() {
        let (_dir, conn) = db("api-reads-card-stats-stall");
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id) VALUES (1001, 'q-oracle-1001')",
            [],
        )
        .expect("card");
        for index in 0..600 {
            let token = format!("q-skip-{index}");
            submittable(&conn, &token, &[&[]]);
            clear_seat(&conn, &token, false);
        }
        submittable(&conn, "q-eligible", &[&[]]);
        let games = card_stats_candidates(&conn, ENQUEUE_LIMIT, AFTER_THE_HOLD).expect("read");
        let subjects: Vec<String> = games
            .iter()
            .map(|game| format!("{}:{}", game.match_token, game.mtgo_game_id))
            .collect();
        assert_eq!(subjects, vec!["q-eligible:901".to_string()]);
        assert!(mymtgo_api::card_stats_payload(&games[0]).is_some());
    }

    /// The query plan of `sql` with each parameter bound to 500, one detail
    /// a line.
    fn plan_of(conn: &Connection, sql: &str) -> String {
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        let values = vec![500; statement.parameter_count()];
        let rows: Vec<String> = statement
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(3)
            })
            .expect("plan")
            .collect::<Result<_, _>>()
            .expect("rows");
        rows.join(" | ")
    }

    // Archetypes spec section 17: the candidate queries reach the archetype
    // rows, the archetypes and the outbox through their primary keys.
    #[test]
    fn the_candidate_queries_use_the_primary_keys() {
        let (_dir, conn) = db("api-reads-candidate-plans");
        for sql in [REPORT_CANDIDATES_SQL, PENDING_REPORTS_SQL] {
            let plan = plan_of(&conn, sql);
            assert!(
                plan.contains("sqlite_autoindex_match_archetypes_1"),
                "{plan}"
            );
            assert!(plan.contains("INTEGER PRIMARY KEY"), "{plan}");
            assert!(plan.contains("sqlite_autoindex_outbox"), "{plan}");
            assert!(!plan.contains("SCAN lp"), "{plan}");
            assert!(!plan.contains("SCAN lo"), "{plan}");
        }
        let plan = plan_of(&conn, CARD_STATS_CANDIDATES_SQL);
        assert!(
            plan.contains("sqlite_autoindex_match_archetypes_1"),
            "{plan}"
        );
        assert!(plan.contains("INTEGER PRIMARY KEY"), "{plan}");
        assert!(plan.contains("sqlite_autoindex_outbox"), "{plan}");
        for alias in ["SCAN lp", "SCAN lo"] {
            assert!(!plan.contains(alias), "{plan}");
        }
    }

    // ---- Brawl commanders in Arena reports (BR7) ---------------------------

    const BRAWL_PLAYER_COMMANDER: i64 = 100_083_902;
    const BRAWL_OPPONENT_COMMANDER: i64 = 100_073_444;
    const BRAWL_OPPONENT_CARD: i64 = 100_070_001;

    /// The Arena match `q-r` as a Historic Brawl match of `match_type`: the
    /// player's deck holds its commander as a sideboard row, the opponent's
    /// seen cards hold its commander and one other card, both seats have a
    /// commander archetype, and `match_commanders` has both commanders. The
    /// player's commander card has a name, the opponent's has none.
    fn brawl_match(conn: &Connection, match_type: &str) {
        rank_match(conn, match_type);
        let signature = decks::signature(&[
            mymtgo_api::report::DeckCard {
                catalog_id: 100_070_002,
                quantity: 1,
                sideboard: false,
            },
            mymtgo_api::report::DeckCard {
                catalog_id: BRAWL_PLAYER_COMMANDER as u32,
                quantity: 1,
                sideboard: true,
            },
        ]);
        conn.execute_batch(
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES ('arena-brawl', 1, 'Qbrawl', 'ARENA_HISTORIC_BRAWL', 0, 0, 'file');",
        )
        .expect("deck");
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at)
             VALUES ((SELECT id FROM decks WHERE mtgo_id = 'arena-brawl'), ?1, 0)",
            [&signature],
        )
        .expect("version");
        conn.execute(
            "UPDATE matches SET format = 'ARENA_HISTORIC_BRAWL', game_structure = 'bo1',
                 deck_version_id = (SELECT id FROM deck_versions) WHERE token = 'q-r'",
            [],
        )
        .expect("format");
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches WHERE token = 'q-r'", [], |row| {
                row.get(0)
            })
            .expect("id");
        let game_id: i64 = conn
            .query_row(
                "SELECT id FROM games WHERE match_id = ?1 AND game_number = 1",
                [match_id],
                |row| row.get(0),
            )
            .expect("game");
        for catalog_id in [BRAWL_OPPONENT_COMMANDER, BRAWL_OPPONENT_CARD] {
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id, seen)
                 VALUES (?1, 'opponent', ?2, 1)",
                params![game_id, catalog_id],
            )
            .expect("opponent card");
        }
        for (seat, position, catalog_id) in [
            ("player", 0, BRAWL_PLAYER_COMMANDER),
            ("opponent", 0, BRAWL_OPPONENT_COMMANDER),
        ] {
            conn.execute(
                "INSERT INTO match_commanders (match_id, seat, position, catalog_id)
                 VALUES (?1, ?2, ?3, ?4)",
                params![match_id, seat, position, catalog_id],
            )
            .expect("commander");
        }
        conn.execute(
            "INSERT INTO cards (catalog_id, name) VALUES (?1, 'Q Commander')",
            [BRAWL_PLAYER_COMMANDER],
        )
        .expect("named card");
        conn.execute(
            "INSERT INTO cards (catalog_id) VALUES (?1)",
            [BRAWL_OPPONENT_COMMANDER],
        )
        .expect("unnamed card");
        for (is_opponent, uuid) in [(false, "q-commander-p"), (true, "q-commander-o")] {
            conn.execute(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES (?1, 'Q Commander', 'historic_brawl', 'commander', 0, 0)",
                [uuid],
            )
            .expect("archetype");
            conn.execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES (?1, ?2, (SELECT id FROM archetypes WHERE uuid = ?3), 1.0)",
                params![match_id, is_opponent, uuid],
            )
            .expect("seat");
        }
    }

    fn the_report_of_q_r(conn: &Connection) -> serde_json::Value {
        let input = arena_inputs(conn)
            .into_iter()
            .find(|input| input.match_token == "q-r")
            .expect("candidate");
        report_body(&input)
    }

    #[test]
    fn a_brawl_report_sends_both_commanders() {
        let (_dir, conn) = db("api-reads-brawl-commanders");
        brawl_match(&conn, "unranked");
        let sent = the_report_of_q_r(&conn);
        assert_eq!(
            sent["arena"]["commanders"],
            serde_json::json!({
                "player": [{"arena_id": 83902, "name": "Q Commander"}],
                "opponent": [{"arena_id": 73444, "name": null}]
            })
        );
        let deck = sent["deck"].as_array().expect("deck");
        assert!(deck.contains(&serde_json::json!(
            {"arena_id": 83902, "quantity": 1, "zone": "side"}
        )));
        let opponent = sent["opponent_deck"].as_array().expect("opponent deck");
        assert!(opponent.contains(&serde_json::json!(
            {"arena_id": 73444, "quantity": 1, "zone": "side"}
        )));
        assert!(opponent.contains(&serde_json::json!(
            {"arena_id": 70001, "quantity": 1, "zone": "main"}
        )));
        assert!(!opponent.contains(&serde_json::json!(
            {"arena_id": 73444, "quantity": 1, "zone": "main"}
        )));
        assert_eq!(sent["player_archetype_uuid"], serde_json::Value::Null);
        assert_eq!(sent["opponent_archetype_uuid"], serde_json::Value::Null);
    }

    #[test]
    fn a_brawl_ladder_report_sends_the_constructed_rank() {
        let (_dir, conn) = db("api-reads-brawl-ladder");
        brawl_match(&conn, "ranked");
        conn.execute(
            "UPDATE arena_match_events SET event_name = 'Brawl_Ladder' WHERE match_token = 'q-r'",
            [],
        )
        .expect("event");
        rank_reading(&conn, "constructed", 95, "Gold", 100);
        let sent = the_report_of_q_r(&conn);
        assert_eq!(sent["arena"]["match_type"], "ranked");
        assert_eq!(sent["arena"]["event_name"], "Brawl_Ladder");
        assert_eq!(sent["arena"]["rank"]["ladder"], "constructed");
    }

    #[test]
    fn a_non_commander_arena_report_keeps_null_commanders() {
        let (_dir, conn) = db("api-reads-no-commanders");
        rank_match(&conn, "ranked");
        let sent = the_report_of_q_r(&conn);
        assert_eq!(sent["arena"]["commanders"], serde_json::Value::Null);
    }
}
