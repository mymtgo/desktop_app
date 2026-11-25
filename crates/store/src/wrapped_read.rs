//! The store read behind MyMTGO Wrapped (spec
//! `docs/superpowers/specs/2026-11-24-wrapped-design.md`): the counted
//! year's rows as a plain `wrapped::WrappedInput`, which the pure builder
//! (`wrapped::build`) turns into cards. Read only.
//!
//! The counted set is the dashboard's: listed (not hidden, not detached),
//! complete, outcome won, lost or drawn (unknown outcomes are left out),
//! `started_at` in the half-open window `[start, end)`. Unlike the
//! dashboard, every account on every platform counts (one Wrapped per
//! player), and every source (log, imported, history, manual).
//!
//! Opponent names, account names and match tokens never enter the input.
//!
//! Speed: the matches, games and runs are a fixed number of queries. The
//! decks (cover, key cards, card stats) and the finishes (one per account)
//! take a few queries each, which is fine for a few hundred decks.

use std::collections::{BTreeSet, HashMap};

use ::cards::stats::CardStat;
use ::leagues::standing::StandingFinish;
use ::leagues::{Classification, LeagueKind, shown_record, summarize_limited_run, summarize_run};
use archetypes::metagame::{ListCard, local_key_cards};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use decks::format::{LIMITED_LABEL, UNKNOWN_FORMAT_LABEL};
use decks::stats::Range;
use rusqlite::{Connection, OptionalExtension, params};
use wrapped::{
    ArchetypeLook, Art, InputArchetype, InputCard, InputDeck, InputDeckGames, InputDraft,
    InputFinish, InputGame, InputMatch, InputRun, KeyCard, MatchResult, WrappedInput, YearWindow,
};

use crate::StoreError;
use crate::card_stats::{CardStatsFilter, card_stats_of_versions, deck_prior_of_versions};
use crate::deck_pages::{header_sql, summary_row, with_commander_cover};
use crate::decks::{CardRow, card_infos, versions_of};
use crate::league_sync::LeagueOwner;
use crate::leagues::{LeagueMatchRow, RunFilter, card_facts, stored_limited_runs, stored_runs};
use crate::names::game_won;

/// The first year MyMTGO Wrapped exists.
const FIRST_WRAPPED_YEAR: i32 = 2026;

/// Everything the Wrapped builder reads for `year`: the matches started in
/// `window` (every account, every platform) with their games, the decks and
/// opponent archetypes they used, each deck's card numbers, the league and
/// course runs, the tournament finishes and the limited runs. `tz` names
/// finish days; `built_at_ms` is passed through.
pub fn wrapped_input(
    conn: &Connection,
    year: i32,
    window: YearWindow,
    tz: Tz,
    built_at_ms: i64,
) -> Result<WrappedInput, StoreError> {
    let matches = counted_matches(conn, window)?;

    let mut deck_ids: Vec<i64> = Vec::new();
    let mut archetype_ids: Vec<i64> = Vec::new();
    for played in &matches {
        if let Some(id) = played.deck_id
            && !deck_ids.contains(&id)
        {
            deck_ids.push(id);
        }
        if let Some(id) = played.opponent_archetype_id
            && !archetype_ids.contains(&id)
        {
            archetype_ids.push(id);
        }
    }

    let mut decks = Vec::new();
    let mut cards = Vec::new();
    let mut deck_games = Vec::new();
    for deck_id in &deck_ids {
        if let Some(deck) = input_deck(conn, *deck_id)? {
            decks.push(deck);
        }
        let (deck_cards, games) = deck_card_numbers(conn, *deck_id, window)?;
        cards.extend(deck_cards);
        deck_games.push(games);
    }

    let runs_read = runs_in_window(conn, window, tz)?;

    Ok(WrappedInput {
        year,
        window,
        built_at_ms,
        tz,
        first_year: year == FIRST_WRAPPED_YEAR,
        matches,
        decks,
        archetypes: faced_archetypes(conn, &archetype_ids)?,
        cards,
        deck_games,
        runs: runs_read.runs,
        finishes: finishes_in_window(conn, window, tz)?,
        drafts: runs_read.drafts,
    })
}

// ---- Matches --------------------------------------------------------------

/// The counted matches of `?1` to `?2` (half-open), oldest first: listed,
/// complete, outcome known or drawn, any account, any source. With the
/// deck (through the version), the opponent seat's archetype, the
/// imported game counts and how many game rows the match has.
const COUNTED_MATCHES: &str = "
SELECT m.id, m.started_at, m.ended_at, m.format, v.deck_id, m.outcome, ma.archetype_id,
       m.games_won_imported, m.games_lost_imported
FROM listed_matches m
LEFT JOIN deck_versions v ON v.id = m.deck_version_id
LEFT JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
WHERE m.state = 'complete' AND m.outcome IN ('win', 'loss', 'draw')
  AND m.started_at >= ?1 AND m.started_at < ?2
ORDER BY m.started_at, m.id";

/// The games of the counted matches (`COUNTED_MATCHES`), in game order.
const COUNTED_GAMES: &str = "
SELECT g.match_id, g.result, g.on_play, g.local_mulligans, g.turn_count
FROM listed_matches m
JOIN games g ON g.match_id = m.id
WHERE m.state = 'complete' AND m.outcome IN ('win', 'loss', 'draw')
  AND m.started_at >= ?1 AND m.started_at < ?2
ORDER BY g.match_id, g.game_number";

/// The counted matches of the window, oldest first, each with its games.
fn counted_matches(conn: &Connection, window: YearWindow) -> Result<Vec<InputMatch>, StoreError> {
    let mut games: HashMap<i64, Vec<InputGame>> = HashMap::new();
    let mut stmt = conn.prepare_cached(COUNTED_GAMES)?;
    let rows = stmt.query_map(params![window.start_ms, window.end_ms], |row| {
        let result: String = row.get(1)?;
        let mulligans: Option<i64> = row.get(3)?;
        let turns: Option<i64> = row.get(4)?;
        Ok((
            row.get::<_, i64>(0)?,
            InputGame {
                won: game_won(&result),
                on_play: row.get(2)?,
                mulligans: to_count(mulligans.unwrap_or(0)),
                turns: turns.map(to_count),
            },
        ))
    })?;
    for row in rows {
        let (match_id, game) = row?;
        games.entry(match_id).or_default().push(game);
    }

    let mut matches = Vec::new();
    let mut stmt = conn.prepare_cached(COUNTED_MATCHES)?;
    let mut rows = stmt.query(params![window.start_ms, window.end_ms])?;
    while let Some(row) = rows.next()? {
        let match_id: i64 = row.get(0)?;
        let format: Option<String> = row.get(3)?;
        let outcome: String = row.get(5)?;
        let won_imported: Option<i64> = row.get(7)?;
        let lost_imported: Option<i64> = row.get(8)?;
        let Some(result) = match_result(&outcome) else {
            continue;
        };
        let match_games = games.remove(&match_id).unwrap_or_default();
        let game_count = if match_games.is_empty() {
            to_count(won_imported.unwrap_or(0)) + to_count(lost_imported.unwrap_or(0))
        } else {
            match_games.len() as u32
        };
        matches.push(InputMatch {
            started_at_ms: row.get(1)?,
            ended_at_ms: row.get(2)?,
            format_label: match_format_label(format.as_deref()),
            limited: format
                .as_deref()
                .is_some_and(decks::is_limited_match_format),
            deck_id: row.get(4)?,
            result,
            opponent_archetype_id: row.get(6)?,
            game_count,
            games: match_games,
        });
    }
    Ok(matches)
}

/// A stored match outcome as a result; `None` for unknown.
fn match_result(outcome: &str) -> Option<MatchResult> {
    match outcome {
        "win" => Some(MatchResult::Win),
        "loss" => Some(MatchResult::Loss),
        "draw" => Some(MatchResult::Draw),
        _ => None,
    }
}

/// A match's format as the Formats card names it (`decks::format_breakdown`):
/// "Limited" for every limited match, "Unknown" without a format, else the
/// display format (`Modern`).
fn match_format_label(format: Option<&str>) -> String {
    match format {
        Some(code) if decks::is_limited_match_format(code) => LIMITED_LABEL.to_string(),
        Some(code) if !code.is_empty() => decks::display_format(code),
        _ => UNKNOWN_FORMAT_LABEL.to_string(),
    }
}

/// A stored count as `u32`: negative reads as 0.
fn to_count(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

/// Comma separated colour letters (`U,R`, any order or case) as a WUBRG
/// list; empty for none.
fn color_list(identity: Option<&str>) -> Vec<String> {
    let normal = ::cards::normalize_colors(identity).unwrap_or_default();
    let mut colors = Vec::new();
    for letter in normal.split(',') {
        let letter = letter.trim();
        if !letter.is_empty() {
            colors.push(letter.to_string());
        }
    }
    colors
}

// ---- Decks ----------------------------------------------------------------

/// The deck as Wrapped shows it: its name, format, colours and cover (the
/// deck pages' `DeckSummary`, commander art included), plus up to five key
/// cards of its latest list. `None` when the deck row is gone.
fn input_deck(conn: &Connection, deck_id: i64) -> Result<Option<InputDeck>, StoreError> {
    let found = conn
        .prepare_cached(&header_sql())?
        .query_row([deck_id], summary_row)
        .optional()?;
    let Some(mut summary) = found else {
        return Ok(None);
    };
    with_commander_cover(conn, &mut summary)?;
    let format_label =
        if decks::is_limited(&summary.format) || decks::is_limited_match_format(&summary.format) {
            LIMITED_LABEL.to_string()
        } else {
            summary.format_label.clone()
        };
    Ok(Some(InputDeck {
        id: summary.id,
        name: summary.name,
        format_label,
        colors: color_list(summary.color_identity.as_deref()),
        // The same three values the deck commands hand the UI
        // (`DeckView` `cover_art`, `local_cover_art`, `custom_cover_art`).
        art: Art {
            remote: summary.cover_art,
            local: summary.local_cover_art,
            custom: summary.cover_file,
        },
        key_cards: deck_key_cards(conn, deck_id)?,
    }))
}

/// Up to five key cards of the deck's latest version's main deck
/// (`archetypes::metagame::local_key_cards`), each with its full image.
fn deck_key_cards(conn: &Connection, deck_id: i64) -> Result<Vec<KeyCard>, StoreError> {
    let versions = versions_of(conn, deck_id)?;
    let Some(latest) = versions.last() else {
        return Ok(Vec::new());
    };
    let main: Vec<decks::DeckCard> = decks::decode_signature(&latest.signature)
        .into_iter()
        .filter(|card| !card.sideboard)
        .collect();
    let ids: Vec<u32> = main.iter().map(|card| card.catalog_id).collect();
    let known = card_infos(conn, &ids)?;
    let mut list = Vec::new();
    for card in &main {
        let info = known.get(&card.catalog_id);
        list.push(ListCard {
            catalog_id: i64::from(card.catalog_id),
            name: info.and_then(|info| info.name.clone()),
            card_type: info.and_then(|info| info.card_type.clone()),
            quantity: card.quantity,
            has_image: info.is_some_and(|info| info.image.is_some() || info.local_image.is_some()),
        });
    }
    Ok(key_cards_of(&local_key_cards(&list), &known))
}

/// The key cards of these catalog ids (from `local_key_cards`), in order,
/// each with its full image. An id with no card row is left out.
fn key_cards_of(picked: &[i64], known: &HashMap<u32, CardRow>) -> Vec<KeyCard> {
    let mut out = Vec::new();
    for catalog_id in picked {
        let Some(card) = u32::try_from(*catalog_id)
            .ok()
            .and_then(|id| known.get(&id))
        else {
            continue;
        };
        out.push(KeyCard {
            name: card.name.clone().unwrap_or_default(),
            image: full_image(card),
        });
    }
    out
}

/// A card's full image.
fn full_image(card: &CardRow) -> Art {
    Art {
        remote: card.image.clone(),
        local: card.local_image.clone(),
        custom: None,
    }
}

/// A card's art crop.
fn art_crop(card: &CardRow) -> Art {
    Art {
        remote: card.art_crop.clone(),
        local: card.local_art_crop.clone(),
        custom: None,
    }
}

/// The deck's games of the window: games with a known opening hand and
/// postboard games with card rows (`?1` the deck, `?2` to `?3` the window,
/// half-open). The same matches the card stats read: listed and complete.
const DECK_HAND_GAMES: &str = "
SELECT COALESCE(SUM(EXISTS (SELECT 1 FROM game_cards gc
           WHERE gc.game_id = g.id AND gc.side = 'local' AND gc.starting_hand > 0)), 0),
       COALESCE(SUM(g.game_number > 1 AND EXISTS (SELECT 1 FROM game_cards gc
           WHERE gc.game_id = g.id AND gc.side = 'local')), 0)
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
JOIN games g ON g.match_id = m.id
WHERE v.deck_id = ?1 AND m.state = 'complete'
  AND m.started_at >= ?2 AND m.started_at < ?3";

/// The deck's card numbers over the window, every version together (the
/// card stats read, `card_stats_of_versions`), and its baselines (its game
/// win rate, `deck_prior_of_versions`, and the games behind the opening
/// hand and sideboard numbers). The card stats count every account's
/// matches: they read by version, with no account filter.
fn deck_card_numbers(
    conn: &Connection,
    deck_id: i64,
    window: YearWindow,
) -> Result<(Vec<InputCard>, InputDeckGames), StoreError> {
    let version_ids: Vec<i64> = versions_of(conn, deck_id)?
        .iter()
        .map(|version| version.id)
        .collect();
    let filter = CardStatsFilter {
        // The card stats window is inclusive: the last millisecond before
        // the end.
        timeframe: Range {
            from_ms: window.start_ms,
            to_ms: window.end_ms - 1,
        },
        ..CardStatsFilter::all()
    };
    let stats = card_stats_of_versions(conn, &version_ids, &filter)?;
    let prior = deck_prior_of_versions(conn, &version_ids, &filter)?;
    let (opening, postboard): (i64, i64) = conn
        .prepare_cached(DECK_HAND_GAMES)?
        .query_row(params![deck_id, window.start_ms, window.end_ms], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;

    let ids: Vec<u32> = stats.iter().map(|stat| stat.catalog_id).collect();
    let known = card_infos(conn, &ids)?;
    let cards = stats
        .iter()
        .map(|stat| input_card(deck_id, stat, known.get(&stat.catalog_id)))
        .collect();
    let games = InputDeckGames {
        deck_id,
        games: prior.games,
        won: prior.wins,
        opening_hand_games: to_count(opening),
        postboard_games: to_count(postboard),
    };
    Ok((cards, games))
}

/// One card stat row as Wrapped reads it. `known` is the shown printing's
/// card row, for the art crop.
fn input_card(deck_id: i64, stat: &CardStat, known: Option<&CardRow>) -> InputCard {
    InputCard {
        deck_id,
        name: stat.name.clone(),
        is_land: stat
            .card_type
            .as_deref()
            .is_some_and(|card_type| card_type.contains("Land")),
        cast_games: stat.cast_games,
        cast_won: stat.cast_won,
        total_cast: stat.total_cast,
        opening_hand_games: stat.kept_games,
        sided_in_games: stat.sided_in_games,
        art: known.map(art_crop).unwrap_or_default(),
        image: Art {
            remote: stat.image.clone(),
            local: stat.local_image.clone(),
            custom: None,
        },
    }
}

// ---- Archetypes -----------------------------------------------------------

/// The opponent archetypes faced, in the order first faced, each with the
/// look of the metagame tile's local path: up to five key cards of its
/// first stored list (`tile_list_cards`, `local_key_cards`) and the first
/// key card's art. An archetype without a list has an empty look.
fn faced_archetypes(
    conn: &Connection,
    archetype_ids: &[i64],
) -> Result<Vec<InputArchetype>, StoreError> {
    let lists = crate::metagame::tile_list_cards(conn, archetype_ids)?;
    let mut picks: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut ids: Vec<u32> = Vec::new();
    for (archetype_id, list) in &lists {
        let picked = local_key_cards(list);
        for catalog_id in &picked {
            if let Ok(id) = u32::try_from(*catalog_id) {
                ids.push(id);
            }
        }
        picks.insert(*archetype_id, picked);
    }
    let known = card_infos(conn, &ids)?;

    let mut out = Vec::new();
    let mut stmt =
        conn.prepare_cached("SELECT name, color_identity FROM archetypes WHERE id = ?1")?;
    for archetype_id in archetype_ids {
        let found: Option<(String, Option<String>)> = stmt
            .query_row([archetype_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let Some((name, identity)) = found else {
            continue;
        };
        let picked = picks.get(archetype_id).cloned().unwrap_or_default();
        let cards = key_cards_of(&picked, &known);
        let art = picked
            .first()
            .and_then(|id| u32::try_from(*id).ok())
            .and_then(|id| known.get(&id))
            .map(art_crop)
            .unwrap_or_default();
        out.push(InputArchetype {
            id: *archetype_id,
            name,
            look: ArchetypeLook {
                art,
                colors: color_list(identity.as_deref()),
                cards,
            },
        });
    }
    Ok(out)
}

// ---- Runs, drafts, finishes -----------------------------------------------

/// The runs of the window and the limited ones as drafts.
struct RunsRead {
    runs: Vec<InputRun>,
    drafts: Vec<InputDraft>,
}

/// Every account, and the runs with no account.
fn every_owner(conn: &Connection) -> Result<Vec<LeagueOwner>, StoreError> {
    let mut owners = vec![LeagueOwner::Nobody];
    for id in account_ids(conn)? {
        owners.push(LeagueOwner::Account(id));
    }
    Ok(owners)
}

fn account_ids(conn: &Connection) -> Result<Vec<i64>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT id FROM accounts ORDER BY id")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// Whether `at_ms` is inside the window (half-open).
fn in_window(window: YearWindow, at_ms: i64) -> bool {
    at_ms >= window.start_ms && at_ms < window.end_ms
}

/// A run's shown record: its complete matches won and lost, each raised to
/// its Arena course's stated count (`leagues::shown_record`).
fn run_record(matches: &[LeagueMatchRow], course_record: Option<(u32, u32)>) -> (u32, u32) {
    let mut wins = 0;
    let mut losses = 0;
    for row in matches.iter().filter(|row| row.state == "complete") {
        match row.outcome.as_str() {
            "win" => wins += 1,
            "loss" => losses += 1,
            _ => {}
        }
    }
    shown_record(course_record, (wins, losses))
}

/// The constructed and limited runs (every account, every source) started
/// in the window. A trophy is the run's `Classification::Trophy` (the run
/// cards' rule). Each limited run is also a draft, with its deck's colour
/// pair (`limited::run_deck_colours` of its registered decks).
fn runs_in_window(conn: &Connection, window: YearWindow, tz: Tz) -> Result<RunsRead, StoreError> {
    let owners = every_owner(conn)?;
    let mut runs = Vec::new();
    let mut drafts = Vec::new();

    let constructed = stored_runs(
        conn,
        &RunFilter {
            owners: owners.clone(),
            kinds: vec![LeagueKind::Constructed],
            deck_id: None,
            run_key: None,
        },
    )?;
    for run in constructed
        .iter()
        .filter(|run| in_window(window, run.started_at))
    {
        let summary = summarize_run(&run.run_input(), None, tz);
        let (wins, losses) = run_record(&run.matches, run.course_record);
        runs.push(InputRun {
            deck_id: run.deck_id,
            limited: false,
            wins,
            losses,
            trophy: summary.classification == Classification::Trophy,
        });
    }

    let limited = stored_limited_runs(
        conn,
        &RunFilter {
            owners,
            kinds: Vec::new(),
            deck_id: None,
            run_key: None,
        },
    )?;
    let counted: Vec<_> = limited
        .runs
        .iter()
        .filter(|run| in_window(window, run.run.started_at))
        .collect();
    let mut ids = BTreeSet::new();
    for run in &counted {
        for deck in &run.registered {
            for card in &deck.cards {
                ids.insert(card.catalog_id);
            }
        }
    }
    let facts = card_facts(conn, &ids)?;
    for run in counted {
        let summary = summarize_limited_run(&run.run_input(), run.run.round_count(), tz);
        let (wins, losses) = run_record(&run.matches, run.course_record);
        runs.push(InputRun {
            deck_id: None,
            limited: true,
            wins,
            losses,
            trophy: summary.classification == Classification::Trophy,
        });
        drafts.push(InputDraft {
            colour_pair: ::limited::run_deck_colours(&run.registered, &facts)
                .filter(|colours| !colours.is_empty())
                .map(|colours| ::limited::colour_combination_name(&colours)),
            wins,
            losses,
        });
    }
    Ok(RunsRead { runs, drafts })
}

/// Every account's tournament standings (this device's only) that finished
/// inside the window. `top_cut` for a trophy or a top cut finish.
fn finishes_in_window(
    conn: &Connection,
    window: YearWindow,
    tz: Tz,
) -> Result<Vec<InputFinish>, StoreError> {
    let mut finishes = Vec::new();
    for account_id in account_ids(conn)? {
        for standing in crate::tournament_sync::standings(conn, account_id)? {
            let (Some(finish), Some(finished_at)) = (standing.finish, standing.finished_at) else {
                continue;
            };
            if !in_window(window, finished_at) {
                continue;
            }
            let event = match standing.name.as_deref() {
                Some(name) if !name.is_empty() => name.to_string(),
                _ => match standing.format.as_deref() {
                    Some(code) if !code.is_empty() => {
                        format!("{} event", decks::display_format(code))
                    }
                    _ => "Tournament".to_string(),
                },
            };
            let place = place_label(finish, standing.place, standing.cut_size);
            finishes.push(InputFinish {
                top_cut: matches!(finish, StandingFinish::Trophied | StandingFinish::TopCut),
                text: format!("{place}, {event}, {}", local_day(finished_at, tz)),
                finished_at_ms: finished_at,
            });
        }
    }
    finishes.sort_by_key(|finish| finish.finished_at_ms);
    Ok(finishes)
}

/// The place part of a finish's text: "1st" for a trophy, "2nd" for the
/// final's loser, else the smallest cut holding the place ("Top 4", "Top
/// 8"), else the cut size; outside the cut the place ("17th") or
/// "Finished".
fn place_label(finish: StandingFinish, place: Option<i64>, cut_size: Option<i64>) -> String {
    let place = place.filter(|place| *place > 0).map(to_count);
    match finish {
        StandingFinish::Trophied => "1st".to_string(),
        StandingFinish::TopCut => match (place, cut_size) {
            (Some(2), _) => "2nd".to_string(),
            (Some(place), _) => format!("Top {}", place.next_power_of_two()),
            (None, Some(cut)) if cut > 0 => format!("Top {cut}"),
            _ => "Top cut".to_string(),
        },
        _ => match place {
            Some(place) => ordinal(place),
            None => "Finished".to_string(),
        },
    }
}

/// `1st`, `2nd`, `3rd`, `4th`, `11th`, `21st`.
fn ordinal(number: u32) -> String {
    let suffix = match (number % 10, number % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{number}{suffix}")
}

/// The local day of `at_ms` in `tz`, like "14 Sep".
fn local_day(at_ms: i64, tz: Tz) -> String {
    match DateTime::<Utc>::from_timestamp_millis(at_ms) {
        Some(at) => at.with_timezone(&tz).format("%-d %b").to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{account, arena_account, db, hide, insert_manual_league};

    const TZ: Tz = chrono_tz::Europe::London;
    const DAY_MS: i64 = 24 * 60 * 60 * 1000;
    /// A built time in December 2026.
    const BUILT_AT: i64 = 1_796_000_000_000;

    fn window() -> YearWindow {
        wrapped::year_window(2026, TZ)
    }

    /// A match day inside the window: `days` after 1 January, at noon.
    fn day(days: i64) -> i64 {
        window().start_ms + days * DAY_MS + 12 * 60 * 60 * 1000
    }

    /// A complete match; returns its row id.
    #[allow(clippy::too_many_arguments)]
    fn insert_match(
        conn: &Connection,
        token: &str,
        account_id: i64,
        source: &str,
        format: &str,
        outcome: &str,
        started_at: i64,
        version_id: Option<i64>,
    ) -> i64 {
        conn.query_row(
            "INSERT INTO matches (token, account_id, source, format, match_type, state, outcome,
                 started_at, ended_at, last_activity_at, revision, derived_version,
                 deck_version_id)
             VALUES (?1, ?2, ?3, ?4, 'Constructed', 'complete', ?5, ?6, ?6 + 1200000,
                 ?6 + 1200000, 1, 0, ?7)
             RETURNING id",
            params![
                token, account_id, source, format, outcome, started_at, version_id
            ],
            |row| row.get(0),
        )
        .expect("a match")
    }

    /// A game of match `match_id`; returns its row id.
    fn insert_game(conn: &Connection, match_id: i64, number: u32, result: &str) -> i64 {
        conn.query_row(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                 local_mulligans, turn_count)
             VALUES (?1, ?2, ?3, 'win', ?2 % 2, ?2 - 1, 6 + ?2) RETURNING id",
            params![match_id, number, result],
            |row| row.get(0),
        )
        .expect("a game")
    }

    fn insert_card(conn: &Connection, catalog_id: u32, name: &str, card_type: &str) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity, image, art_crop,
                 local_image, local_art_crop)
             VALUES (?1, 'oracle-' || ?1, ?2, ?3, 'R', 'https://img/' || ?1,
                 'https://art/' || ?1, ?1 || '.jpg', ?1 || '_art.jpg')",
            params![catalog_id, name, card_type],
        )
        .expect("a card");
    }

    /// A deck with one version listing `cards` (catalog id, copies) in the
    /// main deck; returns (deck id, version id).
    fn insert_deck(
        conn: &Connection,
        name: &str,
        format: &str,
        cards: &[(u32, u32)],
    ) -> (i64, i64) {
        let deck_id: i64 = conn
            .query_row(
                "INSERT INTO decks (mtgo_id, name, format, color_identity, cover_catalog_id,
                     created_at, updated_at, source)
                 VALUES ('q-' || ?1, ?1, ?2, 'R', ?3, 0, 0, 'file') RETURNING id",
                params![name, format, cards.first().map(|card| card.0)],
                |row| row.get(0),
            )
            .expect("a deck");
        let list: Vec<decks::DeckCard> = cards
            .iter()
            .map(|(catalog_id, quantity)| decks::DeckCard {
                catalog_id: *catalog_id,
                quantity: *quantity,
                sideboard: false,
            })
            .collect();
        let version_id: i64 = conn
            .query_row(
                "INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES (?1, ?2, 0) RETURNING id",
                params![deck_id, decks::signature(&list)],
                |row| row.get(0),
            )
            .expect("a version");
        (deck_id, version_id)
    }

    fn input(conn: &Connection) -> WrappedInput {
        wrapped_input(conn, 2026, window(), TZ, BUILT_AT).expect("wrapped input")
    }

    #[test]
    fn the_counted_set_is_the_dashboards_over_every_account() {
        let (_dir, conn) = db("wrapped-counted");
        let mtgo = account(&conn, "Qlocal");
        let arena = arena_account(&conn);
        insert_match(&conn, "q-win", mtgo, "log", "CMODERN", "win", day(10), None);
        insert_match(
            &conn,
            "q-hidden",
            mtgo,
            "log",
            "CMODERN",
            "win",
            day(11),
            None,
        );
        hide(&conn, "q-hidden");
        insert_match(
            &conn,
            "q-unknown",
            mtgo,
            "log",
            "CMODERN",
            "unknown",
            day(12),
            None,
        );
        insert_match(
            &conn,
            "q-draw",
            mtgo,
            "manual",
            "CMODERN",
            "draw",
            day(13),
            None,
        );
        insert_match(
            &conn,
            "q-arena",
            arena,
            "log",
            "ARENA_HISTORIC_BRAWL",
            "loss",
            day(14),
            None,
        );
        let end = window().end_ms;
        insert_match(&conn, "q-at-end", mtgo, "log", "CMODERN", "win", end, None);
        insert_match(
            &conn,
            "q-before-end",
            mtgo,
            "log",
            "DFRAFRAFRA",
            "win",
            end - 1,
            None,
        );
        let start = window().start_ms;
        insert_match(
            &conn,
            "q-before-start",
            mtgo,
            "log",
            "CMODERN",
            "win",
            start - 1,
            None,
        );
        insert_match(
            &conn,
            "q-at-start",
            mtgo,
            "history",
            "CMODERN",
            "loss",
            start,
            None,
        );
        conn.execute(
            "UPDATE matches SET state = 'abandoned' WHERE token = 'q-win'",
            [],
        )
        .expect("abandon");
        insert_match(
            &conn,
            "q-win-2",
            mtgo,
            "imported",
            "CPIONEER",
            "win",
            day(10),
            None,
        );

        let read = input(&conn);
        let counted: Vec<(i64, MatchResult, &str, bool)> = read
            .matches
            .iter()
            .map(|played| {
                (
                    played.started_at_ms,
                    played.result,
                    played.format_label.as_str(),
                    played.limited,
                )
            })
            .collect();
        assert_eq!(
            counted,
            vec![
                (start, MatchResult::Loss, "Modern", false),
                (day(10), MatchResult::Win, "Pioneer", false),
                (day(13), MatchResult::Draw, "Modern", false),
                (day(14), MatchResult::Loss, "Historic Brawl", false),
                (end - 1, MatchResult::Win, "Limited", true),
            ]
        );
        assert!(read.first_year);
        assert_eq!(read.year, 2026);
        assert_eq!(read.built_at_ms, BUILT_AT);
    }

    #[test]
    fn games_come_from_rows_or_the_imported_counts() {
        let (_dir, conn) = db("wrapped-games");
        let local = account(&conn, "Qlocal");
        let live = insert_match(
            &conn,
            "q-live",
            local,
            "log",
            "CMODERN",
            "win",
            day(1),
            None,
        );
        insert_game(&conn, live, 1, "loss");
        insert_game(&conn, live, 2, "win");
        insert_game(&conn, live, 3, "win");
        let imported = insert_match(
            &conn,
            "q-imp",
            local,
            "imported",
            "CMODERN",
            "win",
            day(2),
            None,
        );
        conn.execute(
            "UPDATE matches SET games_won_imported = 2, games_lost_imported = 1 WHERE id = ?1",
            [imported],
        )
        .expect("imported counts");

        let read = input(&conn);
        assert_eq!(read.matches[0].game_count, 3);
        assert_eq!(
            read.matches[0].games,
            vec![
                InputGame {
                    won: Some(false),
                    on_play: Some(true),
                    mulligans: 0,
                    turns: Some(7)
                },
                InputGame {
                    won: Some(true),
                    on_play: Some(false),
                    mulligans: 1,
                    turns: Some(8)
                },
                InputGame {
                    won: Some(true),
                    on_play: Some(true),
                    mulligans: 2,
                    turns: Some(9)
                },
            ]
        );
        assert_eq!(read.matches[1].game_count, 3);
        assert!(read.matches[1].games.is_empty());
    }

    #[test]
    fn a_deck_brings_its_cover_key_cards_and_card_numbers() {
        let (_dir, conn) = db("wrapped-deck");
        let local = account(&conn, "Qlocal");
        insert_card(&conn, 101, "Qbolt", "Instant");
        insert_card(&conn, 102, "Qmountain", "Basic Land");
        insert_card(&conn, 103, "Qguide", "Creature");
        let (deck_id, version) =
            insert_deck(&conn, "Q Burn", "CMODERN", &[(101, 4), (102, 20), (103, 2)]);
        conn.execute("UPDATE decks SET cover_file = '1-2.png'", [])
            .expect("cover file");

        let won = insert_match(
            &conn,
            "q-1",
            local,
            "log",
            "CMODERN",
            "win",
            day(5),
            Some(version),
        );
        let lost = insert_match(
            &conn,
            "q-2",
            local,
            "log",
            "CMODERN",
            "loss",
            day(6),
            Some(version),
        );
        for (game, cast, hand) in [
            (insert_game(&conn, won, 1, "win"), 2, 1),
            (insert_game(&conn, won, 2, "win"), 1, 0),
            (insert_game(&conn, lost, 1, "loss"), 0, 1),
        ] {
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, opening_hand,
                     starting_hand, cast)
                 VALUES (?1, 'local', 101, 4, ?2, ?2, ?3),
                        (?1, 'local', 102, 20, 2, 2, 0)",
                params![game, hand, cast],
            )
            .expect("card rows");
        }
        // A sided in card in game 2.
        let game_two: i64 = conn
            .query_row(
                "SELECT id FROM games WHERE match_id = ?1 AND game_number = 2",
                [won],
                |row| row.get(0),
            )
            .expect("game 2");
        conn.execute(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, sided_in, cast)
             VALUES (?1, 'local', 103, 1, 1, 1)",
            [game_two],
        )
        .expect("sided in row");

        let read = input(&conn);
        assert_eq!(read.decks.len(), 1);
        let deck = &read.decks[0];
        assert_eq!(deck.id, deck_id);
        assert_eq!(deck.name, "Q Burn");
        assert_eq!(deck.format_label, "Modern");
        assert_eq!(deck.colors, vec!["R".to_string()]);
        assert_eq!(
            deck.art,
            Art {
                remote: Some("https://art/101".to_string()),
                local: Some("101_art.jpg".to_string()),
                custom: Some("1-2.png".to_string()),
            }
        );
        let names: Vec<&str> = deck
            .key_cards
            .iter()
            .map(|card| card.name.as_str())
            .collect();
        assert_eq!(names, vec!["Qbolt", "Qguide"]);
        assert_eq!(
            deck.key_cards[0].image,
            Art {
                remote: Some("https://img/101".to_string()),
                local: Some("101.jpg".to_string()),
                custom: None,
            }
        );

        let bolt = read
            .cards
            .iter()
            .find(|card| card.name == "Qbolt")
            .expect("bolt");
        assert_eq!(
            bolt,
            &InputCard {
                deck_id,
                name: "Qbolt".to_string(),
                is_land: false,
                cast_games: 2,
                cast_won: 2,
                total_cast: 3,
                opening_hand_games: 2,
                sided_in_games: 0,
                art: Art {
                    remote: Some("https://art/101".to_string()),
                    local: Some("101_art.jpg".to_string()),
                    custom: None,
                },
                image: Art {
                    remote: Some("https://img/101".to_string()),
                    local: Some("101.jpg".to_string()),
                    custom: None,
                },
            }
        );
        let mountain = read
            .cards
            .iter()
            .find(|card| card.name == "Qmountain")
            .expect("mountain");
        assert!(mountain.is_land);
        let guide = read
            .cards
            .iter()
            .find(|card| card.name == "Qguide")
            .expect("guide");
        assert_eq!(guide.sided_in_games, 1);

        assert_eq!(
            read.deck_games,
            vec![InputDeckGames {
                deck_id,
                games: 3,
                won: 2,
                opening_hand_games: 3,
                postboard_games: 1,
            }]
        );
    }

    #[test]
    fn opponent_archetypes_bring_their_look() {
        let (_dir, conn) = db("wrapped-archetypes");
        let local = account(&conn, "Qlocal");
        insert_card(&conn, 201, "Qgoyf", "Creature");
        insert_card(&conn, 202, "Qforest", "Basic Land");
        let archetype: i64 = conn
            .query_row(
                "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                     updated_at)
                 VALUES ('q-arch', 'Q Midrange', 'modern', 'G,B', 'manual', 0, 0) RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("archetype");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES (?1, 1, 201, 4, 0), (?1, 1, 202, 10, 0)",
            [archetype],
        )
        .expect("archetype list");
        let played = insert_match(&conn, "q-1", local, "log", "CMODERN", "win", day(3), None);
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES (?1, 1, ?2, 1.0)",
            params![played, archetype],
        )
        .expect("faced");

        let read = input(&conn);
        assert_eq!(read.matches[0].opponent_archetype_id, Some(archetype));
        assert_eq!(
            read.archetypes,
            vec![InputArchetype {
                id: archetype,
                name: "Q Midrange".to_string(),
                look: ArchetypeLook {
                    art: Art {
                        remote: Some("https://art/201".to_string()),
                        local: Some("201_art.jpg".to_string()),
                        custom: None,
                    },
                    colors: vec!["B".to_string(), "G".to_string()],
                    cards: vec![KeyCard {
                        name: "Qgoyf".to_string(),
                        image: Art {
                            remote: Some("https://img/201".to_string()),
                            local: Some("201.jpg".to_string()),
                            custom: None,
                        },
                    }],
                },
            }]
        );
    }

    #[test]
    fn a_five_win_league_is_a_trophy_run() {
        let (_dir, conn) = db("wrapped-trophy");
        let local = account(&conn, "Qlocal");
        insert_card(&conn, 301, "Qbolt", "Instant");
        let (deck_id, version) = insert_deck(&conn, "Q Burn", "CMODERN", &[(301, 4)]);
        insert_manual_league(
            &conn,
            1,
            Some(local),
            Some(deck_id),
            "Q League",
            day(20),
            day(20),
        );
        for round in 0..5 {
            let token = format!("q-league-{round}");
            insert_match(
                &conn,
                &token,
                local,
                "manual",
                "CMODERN",
                "win",
                day(20) + round,
                Some(version),
            );
            crate::test_support::link_league_match(&conn, 1, &token);
        }
        // A run started before the year is not read.
        insert_manual_league(
            &conn,
            2,
            Some(local),
            Some(deck_id),
            "Q Old",
            window().start_ms - 1,
            0,
        );

        let read = input(&conn);
        assert_eq!(
            read.runs,
            vec![InputRun {
                deck_id: Some(deck_id),
                limited: false,
                wins: 5,
                losses: 0,
                trophy: true,
            }]
        );
        assert!(read.drafts.is_empty());
    }

    #[test]
    fn finishes_read_as_place_event_and_day() {
        assert_eq!(
            place_label(StandingFinish::Trophied, Some(1), Some(8)),
            "1st"
        );
        assert_eq!(place_label(StandingFinish::TopCut, Some(2), Some(8)), "2nd");
        assert_eq!(
            place_label(StandingFinish::TopCut, Some(3), Some(8)),
            "Top 4"
        );
        assert_eq!(
            place_label(StandingFinish::TopCut, Some(7), Some(8)),
            "Top 8"
        );
        assert_eq!(place_label(StandingFinish::TopCut, None, Some(8)), "Top 8");
        assert_eq!(
            place_label(StandingFinish::MissedCut, Some(21), Some(8)),
            "21st"
        );
        assert_eq!(place_label(StandingFinish::Dropped, None, None), "Finished");
        // 14 September 2026, 21:30 UTC is 22:30 in London.
        let at = DateTime::parse_from_rfc3339("2026-09-14T21:30:00Z")
            .expect("time")
            .timestamp_millis();
        assert_eq!(local_day(at, TZ), "14 Sep");
        assert_eq!(local_day(at, chrono_tz::Pacific::Auckland), "15 Sep");
    }

    #[test]
    fn a_realistic_year_builds_a_bundle() {
        let (_dir, conn) = db("wrapped-build");
        let local = account(&conn, "Qlocal");
        let arena = arena_account(&conn);
        insert_card(&conn, 401, "Qbolt", "Instant");
        insert_card(&conn, 402, "Qmountain", "Basic Land");
        let (_deck_id, version) = insert_deck(&conn, "Q Burn", "CMODERN", &[(401, 4), (402, 20)]);
        for index in 0..40_i64 {
            let owner = if index % 4 == 0 { arena } else { local };
            let outcome = if index % 3 == 0 { "loss" } else { "win" };
            let played = insert_match(
                &conn,
                &format!("q-{index}"),
                owner,
                "log",
                "CMODERN",
                outcome,
                day(index * 7),
                Some(version),
            );
            let game = insert_game(&conn, played, 1, outcome);
            conn.execute(
                "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, opening_hand,
                     starting_hand, cast)
                 VALUES (?1, 'local', 401, 4, 1, 1, 1)",
                [game],
            )
            .expect("card row");
        }

        let read = input(&conn);
        assert_eq!(read.matches.len(), 40);
        let bundle = wrapped::build(&read).expect("a bundle");
        assert_eq!(bundle.totals.matches, 40);
    }
}
