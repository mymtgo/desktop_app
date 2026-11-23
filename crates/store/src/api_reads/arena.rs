//! The Arena side of the match report reads (Arena match reports spec
//! sections 4.1, 5 and 11): every completed Arena match with an event name
//! is a candidate once its processing is done, with or without an archetype
//! or a deck version. The MTGO reads in the parent module do not change.

use decks::DeckCard;
use mymtgo_api::report::{ArenaCommander, ArenaCommanders, ArenaRank, ArenaReport};
use mymtgo_api::{MatchArchetypes, MatchReportInput};
use rusqlite::{OptionalExtension, params};

use super::{held_until, report_games, seat_at};
use crate::match_commanders::{commanders_of, is_commander_format};
use crate::{Connection, StoreError};

/// The longest opponent name the API takes.
const MAX_OPPONENT_NAME: usize = 64;

/// The Arena matches to report, at most `?1`, in start order: complete, not
/// manual, an Arena account that has a name to send (`arena_username`'s
/// rule), an event name, the format, best-of and match type the API takes,
/// done processing (a log match is stamped by its game sync, which runs
/// after its deck link, or corrected by the player: `DUE_MATCHES_SQL`'s
/// rule), archetype detected unless detection never takes it (#759: a
/// hidden or detached match is not listed, and a log match with no game
/// sync waits for it), ended at least `SEND_HOLD_MS` ago (`?2` is `held_until(now)`), no
/// `match_report` outbox row. Both seats are optional. Read with each seat's
/// uuid and source.
pub(super) const ARENA_REPORT_CANDIDATES_SQL: &str =
    "SELECT m.id, pa.uuid, pa.source, oa.uuid, oa.source
     FROM matches m
     JOIN accounts ac ON ac.id = m.account_id AND ac.platform = 'arena'
     JOIN arena_match_events e ON e.match_token = m.token
     LEFT JOIN match_archetypes lp ON lp.match_id = m.id AND lp.is_opponent = 0
     LEFT JOIN archetypes pa ON pa.id = lp.archetype_id
     LEFT JOIN match_archetypes lo ON lo.match_id = m.id AND lo.is_opponent = 1
     LEFT JOIN archetypes oa ON oa.id = lo.archetype_id
     WHERE m.state = 'complete' AND m.source <> 'manual'
       AND m.outcome IN ('win', 'loss', 'draw')
       AND (ac.username NOT LIKE 'arena:%' OR NULLIF(ac.display_name, '') IS NOT NULL)
       AND m.format IS NOT NULL AND m.game_structure IN ('bo1', 'bo3')
       AND m.match_type IN ('ranked', 'unranked', 'limited')
       AND (m.source <> 'log' OR m.games_synced_at IS NOT NULL OR m.corrected_at IS NOT NULL)
       AND (m.archetypes_detected_at IS NOT NULL
            OR m.hidden_at IS NOT NULL OR m.detached_at IS NOT NULL
            OR (m.source = 'log' AND m.games_synced_at IS NULL))
       AND COALESCE(m.ended_at, m.last_activity_at) <= ?2
       AND NOT EXISTS (
           SELECT 1 FROM outbox o WHERE o.kind = 'match_report' AND o.subject = m.token)
     ORDER BY m.started_at, m.id
     LIMIT ?1";

/// How many matches `ARENA_REPORT_CANDIDATES_SQL` would read without a limit
/// (`?1` is `held_until(now)`).
pub(super) const ARENA_PENDING_REPORTS_SQL: &str = "SELECT COUNT(*)
     FROM matches m
     JOIN accounts ac ON ac.id = m.account_id AND ac.platform = 'arena'
     JOIN arena_match_events e ON e.match_token = m.token
     WHERE m.state = 'complete' AND m.source <> 'manual'
       AND m.outcome IN ('win', 'loss', 'draw')
       AND (ac.username NOT LIKE 'arena:%' OR NULLIF(ac.display_name, '') IS NOT NULL)
       AND m.format IS NOT NULL AND m.game_structure IN ('bo1', 'bo3')
       AND m.match_type IN ('ranked', 'unranked', 'limited')
       AND (m.source <> 'log' OR m.games_synced_at IS NOT NULL OR m.corrected_at IS NOT NULL)
       AND (m.archetypes_detected_at IS NOT NULL
            OR m.hidden_at IS NOT NULL OR m.detached_at IS NOT NULL
            OR (m.source = 'log' AND m.games_synced_at IS NULL))
       AND COALESCE(m.ended_at, m.last_activity_at) <= ?1
       AND NOT EXISTS (
           SELECT 1 FROM outbox o WHERE o.kind = 'match_report' AND o.subject = m.token)";

/// Up to `limit` Arena matches to report, in start order.
pub(super) fn arena_candidates(
    conn: &Connection,
    limit: u32,
    client_version: &str,
    now_ms: i64,
) -> Result<Vec<MatchReportInput>, StoreError> {
    let mut found: Vec<(i64, MatchArchetypes)> = Vec::new();
    {
        let mut statement = conn.prepare_cached(ARENA_REPORT_CANDIDATES_SQL)?;
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
        if let Some(input) = arena_input(conn, id, client_version, sides)? {
            inputs.push(input);
        }
    }
    Ok(inputs)
}

/// How many Arena matches wait to be queued at `now_ms`.
pub(super) fn arena_pending_to_queue(conn: &Connection, now_ms: i64) -> Result<u32, StoreError> {
    let count: u32 = conn
        .prepare_cached(ARENA_PENDING_REPORTS_SQL)?
        .query_row([held_until(now_ms)], |row| row.get(0))?;
    Ok(count)
}

/// The name an Arena report sends for the local player: the tagged username,
/// or the display name of an untagged account (keyed `arena:<user id>`).
/// `None` when the account has no name to send.
fn arena_username(username: &str, display_name: Option<&str>) -> Option<String> {
    if username.starts_with("arena:") {
        return display_name
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string());
    }
    Some(username.to_string())
}

/// The name an Arena report sends for the opponent: the display name, else a
/// username that is not an `arena:` key; `None` when neither exists or the
/// name is longer than the API takes.
fn arena_opponent_name(username: &str, display_name: Option<&str>) -> Option<String> {
    let name = match display_name.filter(|name| !name.is_empty()) {
        Some(name) => name,
        None if username.starts_with("arena:") => return None,
        None => username,
    };
    if name.chars().count() > MAX_OPPONENT_NAME {
        return None;
    }
    Some(name.to_string())
}

/// The run keys an Arena report sends in MTGO's league keys (spec 4.3):
/// the run's token (`arena:<course_id>`), its `leagues.id` and its
/// `sync_client_id`. All `None` for a match with no run, and for a run keyed
/// by no course (trap 34: it says nothing about the entry).
struct RunKeys {
    token: Option<String>,
    id: Option<i64>,
    client_id: Option<String>,
}

impl RunKeys {
    fn of(id: Option<i64>, token: Option<String>, client_id: Option<String>) -> RunKeys {
        let has_course = token
            .as_deref()
            .is_some_and(|token| leagues::arena_course_id(token).is_some());
        if !has_course {
            return RunKeys {
                token: None,
                id: None,
                client_id: None,
            };
        }
        RunKeys {
            token,
            id,
            client_id,
        }
    }
}

/// One Arena match's report input; `None` when the match is gone or its
/// account has no name to send. The deck is the deck version's signature
/// (constructed), else for a limited match the course deck.
fn arena_input(
    conn: &Connection,
    match_id: i64,
    client_version: &str,
    sides: MatchArchetypes,
) -> Result<Option<MatchReportInput>, StoreError> {
    let row = conn
        .prepare_cached(
            "SELECT m.token, a.username, a.display_name, a.arena_user_id, a.login_id, m.outcome,
                 m.format, m.started_at, o.username, o.display_name, v.signature,
                 m.game_structure, m.match_type, e.event_name, l.id, l.token, l.sync_client_id
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             LEFT JOIN deck_versions v ON v.id = m.deck_version_id
             LEFT JOIN arena_match_events e ON e.match_token = m.token
             LEFT JOIN leagues l ON l.id = m.league_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| {
            let username: String = row.get(1)?;
            let display_name: Option<String> = row.get(2)?;
            let opponent_username: Option<String> = row.get(8)?;
            let opponent_display: Option<String> = row.get(9)?;
            let opponent_name = opponent_username
                .and_then(|name| arena_opponent_name(&name, opponent_display.as_deref()));
            let sent_username = arena_username(&username, display_name.as_deref());
            let event_name: Option<String> = row.get(13)?;
            let best_of: Option<String> = row.get(11)?;
            let match_type: Option<String> = row.get(12)?;
            let run = RunKeys::of(row.get(14)?, row.get(15)?, row.get(16)?);
            let input = MatchReportInput {
                match_token: row.get(0)?,
                username: sent_username.clone().unwrap_or_default(),
                login_id: row.get(4)?,
                player_archetype: sides.player.clone(),
                opponent_archetype: sides.opponent.clone(),
                outcome: row.get(5)?,
                format: row.get(6)?,
                tournament_event_id: None,
                league_token: run.token,
                league_id: run.id,
                league_client_id: run.client_id,
                league_playevent_id: None,
                tournament_token: None,
                tournament_round: None,
                started_at_ms: row.get(7)?,
                client_version: client_version.to_string(),
                opponent_name,
                deck: Vec::new(),
                oracle_cards: Vec::new(),
                games: Vec::new(),
                arena: Some(ArenaReport {
                    arena_user_id: row.get(3)?,
                    event_name: event_name.unwrap_or_default(),
                    best_of: best_of.unwrap_or_default(),
                    match_type: match_type.unwrap_or_default(),
                    season: None,
                    rank: None,
                    commanders: None,
                }),
            };
            let named = sent_username.is_some();
            Ok((named, input, row.get::<_, Option<String>>(10)?))
        })
        .optional()?;
    let Some((true, mut input, signature)) = row else {
        return Ok(None);
    };
    input.deck = match signature.as_deref() {
        Some(signature) => decks::decode_signature(signature),
        None if input
            .arena
            .as_ref()
            .is_some_and(|a| a.match_type == "limited") =>
        {
            arena_limited_deck(conn, match_id)?
        }
        None => Vec::new(),
    };
    input.games = report_games(conn, match_id)?;
    if is_commander_format(input.format.as_deref()) {
        let found = commanders_of(conn, match_id)?;
        let commanders = ArenaCommanders {
            player: arena_commanders(conn, &found.player)?,
            opponent: arena_commanders(conn, &found.opponent)?,
        };
        if let Some(arena) = input.arena.as_mut() {
            arena.commanders = Some(commanders);
        }
    }
    if let Some(arena) = input.arena.as_mut() {
        arena.season = arena_season_at(conn, match_id)?;
        arena.rank = match arena.match_type.as_str() {
            "ranked" => arena_rank_at(conn, match_id, "constructed")?,
            "limited" => arena_rank_at(conn, match_id, "limited")?,
            _ => None,
        };
    }
    Ok(Some(input))
}

/// The commanders of one seat as the report sends them: the Arena id (catalog
/// id minus the offset) and the card name, `None` when the card row is
/// missing or has no name. A catalog id that is no Arena card is left out.
fn arena_commanders(
    conn: &Connection,
    catalog_ids: &[i64],
) -> Result<Vec<ArenaCommander>, StoreError> {
    let mut commanders = Vec::new();
    for catalog_id in catalog_ids {
        let Some(grp) = cards::grp_id_of(*catalog_id) else {
            continue;
        };
        let name: Option<Option<String>> = conn
            .prepare_cached("SELECT name FROM cards WHERE catalog_id = ?1")?
            .query_row([catalog_id], |row| row.get(0))
            .optional()?;
        commanders.push(ArenaCommander {
            arena_id: grp.0,
            name: name.flatten().filter(|name| !name.is_empty()),
        });
    }
    Ok(commanders)
}

/// The deck of an Arena limited match: the main deck and sideboard of the
/// latest snapshot (by log place) of its run's course (`leagues.token` is `arena:<course
/// id>`) read at or before the match's end. The course card ids are catalog
/// ids already (grpId + `ARENA_ID_OFFSET`), sent as stored. Empty when the
/// match has no run, the run has no course (trap 34) or no snapshot came
/// before the match ended.
pub(super) fn arena_limited_deck(
    conn: &Connection,
    match_id: i64,
) -> Result<Vec<DeckCard>, StoreError> {
    let run: Option<(i64, String, i64)> = conn
        .prepare_cached(
            "SELECT m.account_id, l.token, COALESCE(m.ended_at, m.last_activity_at)
             FROM matches m
             JOIN leagues l ON l.id = m.league_id
             WHERE m.id = ?1 AND l.token IS NOT NULL",
        )?
        .query_row([match_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let Some((account_id, token, ended_at)) = run else {
        return Ok(Vec::new());
    };
    let Some(course_id) = leagues::arena_course_id(&token) else {
        return Ok(Vec::new());
    };
    let json: Option<String> = conn
        .prepare_cached(
            "SELECT cards FROM arena_course_observations
             WHERE account_id = ?1 AND course_id = ?2 AND observed_at <= ?3
             ORDER BY arena_log_file_id DESC, byte_offset DESC
             LIMIT 1",
        )?
        .query_row(params![account_id, course_id, ended_at], |row| row.get(0))
        .optional()?;
    let Some(json) = json else {
        return Ok(Vec::new());
    };
    // A snapshot that does not read must not fail the whole report batch.
    let Ok(cards) = serde_json::from_str::<CourseCards>(&json) else {
        return Ok(Vec::new());
    };
    let mut deck = Vec::new();
    for (catalog_id, quantity) in cards.main {
        deck.push(DeckCard {
            catalog_id,
            quantity,
            sideboard: false,
        });
    }
    for (catalog_id, quantity) in cards.side {
        deck.push(DeckCard {
            catalog_id,
            quantity,
            sideboard: true,
        });
    }
    Ok(deck)
}

/// The deck part of a course snapshot's `cards` column (the pool is not
/// read).
#[derive(serde::Deserialize)]
struct CourseCards {
    main: Vec<(u32, u32)>,
    side: Vec<(u32, u32)>,
}

/// The columns of an `arena_rank_observations` row (aliased `o`) that make
/// an `ArenaRank`.
const RANK_COLUMNS: &str = "o.ladder, o.class, o.level, o.step, o.mythic_percentile,
     o.mythic_placement, o.season, o.observed_at";

/// The readings of the match's account placed strictly before the match's
/// first stored Arena event in log order (`?1` is the match id), newest
/// season first, then latest log place: the rule of the `match_rank_readings`
/// view (migration 0013). `{ladder_filter}` is a filter on `o.ladder` or
/// nothing. A match with no stored Arena event has no place to read before,
/// so it gets no reading.
fn readings_before_sql(ladder_filter: &str) -> String {
    format!(
        "SELECT {RANK_COLUMNS}
         FROM matches m
         JOIN arena_log_events f ON f.rowid = (
             SELECT e.rowid FROM arena_log_events e
             WHERE e.match_token = m.token
             ORDER BY e.arena_log_file_id, e.byte_offset
             LIMIT 1)
         JOIN arena_rank_observations o ON o.account_id = m.account_id {ladder_filter}
           AND (o.arena_log_file_id, o.byte_offset) < (f.arena_log_file_id, f.byte_offset)
         WHERE m.id = ?1
         ORDER BY o.season DESC, o.arena_log_file_id DESC, o.byte_offset DESC
         LIMIT 1"
    )
}

fn rank_from_row(row: &rusqlite::Row) -> rusqlite::Result<ArenaRank> {
    Ok(ArenaRank {
        ladder: row.get(0)?,
        class: row.get(1)?,
        level: row.get(2)?,
        step: row.get(3)?,
        mythic_percentile: row.get(4)?,
        mythic_placement: row.get(5)?,
        season: row.get(6)?,
        observed_at_ms: row.get(7)?,
    })
}

/// The rank the match was played at on `ladder` (`constructed` or
/// `limited`): a ranked constructed match uses the reading it stores
/// (`rank_file_id`, `rank_byte_offset`) when it has one, otherwise the
/// latest reading before the match by the view's rule. `None` when no
/// reading came before the match.
pub(super) fn arena_rank_at(
    conn: &Connection,
    match_id: i64,
    ladder: &str,
) -> Result<Option<ArenaRank>, StoreError> {
    if ladder == "constructed" {
        let stored = conn
            .prepare_cached(&format!(
                "SELECT {RANK_COLUMNS}
                 FROM matches m
                 JOIN arena_rank_observations o ON o.arena_log_file_id = m.rank_file_id
                     AND o.byte_offset = m.rank_byte_offset AND o.ladder = 'constructed'
                 WHERE m.id = ?1"
            ))?
            .query_row([match_id], rank_from_row)
            .optional()?;
        if stored.is_some() {
            return Ok(stored);
        }
    }
    let sql = readings_before_sql("AND o.ladder = ?2");
    Ok(conn
        .prepare_cached(&sql)?
        .query_row(params![match_id, ladder], rank_from_row)
        .optional()?)
}

/// The season of the latest reading of any ladder before the match, the
/// newest season winning over a stale answer logged later.
pub(super) fn arena_season_at(conn: &Connection, match_id: i64) -> Result<Option<i64>, StoreError> {
    let sql = readings_before_sql("");
    Ok(conn
        .prepare_cached(&sql)?
        .query_row([match_id], |row| row.get::<_, i64>(6))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untagged_account_sends_its_display_name() {
        assert_eq!(
            arena_username("arena:abc", Some("Qarena")),
            Some("Qarena".to_string())
        );
        assert_eq!(arena_username("arena:abc", None), None);
        assert_eq!(
            arena_username("Qarena#12345", Some("Qarena")),
            Some("Qarena#12345".to_string())
        );
    }

    #[test]
    fn opponent_names_are_never_keys_or_too_long() {
        assert_eq!(
            arena_opponent_name("arena:xyz", Some("Opp")),
            Some("Opp".to_string())
        );
        assert_eq!(arena_opponent_name("arena:xyz", None), None);
        assert_eq!(
            arena_opponent_name("arena:name:Opp", Some("Opp")),
            Some("Opp".to_string())
        );
        assert_eq!(
            arena_opponent_name("arena:xyz", Some(&"x".repeat(65))),
            None
        );
        assert_eq!(
            arena_opponent_name("Opp#1", None),
            Some("Opp#1".to_string())
        );
    }
}
