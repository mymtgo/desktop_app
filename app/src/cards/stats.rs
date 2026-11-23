//! The deck Card Stats tab's commands (cards spec section 9): the card stats
//! payload in 0.x's shape without the community parts (spec ruling 10),
//! "Regenerate" and the trust setting.

use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use store::Connection;
use store::card_stats::{
    ArchetypeOption, CardStatsFilter, DeckWinrate, card_stats, deck_prior, opponent_archetypes,
};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use cards::stats::{Board, CardStat};
use game_detail::Side;

use crate::bridge::lock;
use crate::commands::{
    AppState, CommandError, Startup, off_async_workers, read_settings, ready, storage,
};
use crate::decks::{deck_best_of, platform_best_of, timeframe_ranges};
use crate::events::SettingsChanged;
use crate::logging::Timer;
use crate::platform::BestOfView;
use crate::settings::{CARD_STATS_TRUST, MAX_CARD_STATS_TRUST};

/// Game 1 or postboard games (0.x `card_stats_board`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum BoardKey {
    Game1,
    Postboard,
}

/// On the play or on the draw (0.x `card_stats_play_draw`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum PlayDrawKey {
    Play,
    Draw,
}

/// Whose cards (0.x `card_stats_perspective`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum Perspective {
    #[default]
    Mine,
    Theirs,
}

/// The tab's filters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardStatsQuery {
    /// The page's timeframe key (`alltime` by default, as 0.x).
    pub timeframe: String,
    /// `None` for every game.
    pub board: Option<BoardKey>,
    /// `None` for both.
    pub play_draw: Option<PlayDrawKey>,
    /// An opponent archetype id; `None` for every archetype.
    pub archetype: Option<u32>,
    pub perspective: Perspective,
    /// An Arena kind; `None` for every match. A deck read ignores it for an
    /// MTGO deck.
    #[serde(default)]
    pub best_of: Option<BestOfView>,
}

/// One card's numbers (0.x `GetCardGameStats` row, plus `inferredGames`).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardStatView {
    /// The oracle id, or `#{catalog id}` for a card without card data.
    pub key: String,
    pub oracle_id: Option<String>,
    pub catalog_id: u32,
    pub name: String,
    pub color_identity: Option<String>,
    #[serde(rename = "type")]
    pub card_type: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
    pub is_sideboard: bool,
    pub total_games: u32,
    pub total_possible: u32,
    pub total_kept: u32,
    pub kept_games: u32,
    pub kept_won: u32,
    pub kept_lost: u32,
    pub total_seen: u32,
    pub seen_games: u32,
    pub seen_won: u32,
    pub seen_lost: u32,
    pub total_cast: u32,
    pub cast_games: u32,
    pub cast_won: u32,
    pub cast_lost: u32,
    pub postboard_games: u32,
    pub sided_out_games: u32,
    pub sided_in_games: u32,
    pub total_played: u32,
    pub played_games: u32,
    pub total_kicked: u32,
    pub total_activated: u32,
    pub total_flashback: u32,
    pub total_madness: u32,
    pub total_evoked: u32,
    pub total_warp: u32,
    pub total_free_cast: u32,
    pub total_bargained: u32,
    pub total_dashed: u32,
    pub total_bestowed: u32,
    pub total_replicated: u32,
    pub total_spectacle: u32,
    pub total_rebound: u32,
    pub total_escaped: u32,
    pub total_ninjutsu: u32,
    pub total_suspended: u32,
    pub total_buyback: u32,
    pub total_disturb: u32,
    pub total_foretold: u32,
    pub total_retraced: u32,
    pub total_mayhem: u32,
    pub total_miracle: u32,
    pub total_gifted: u32,
    pub total_casualty: u32,
    pub pregame_revealed_games: u32,
    pub pregame_played_games: u32,
    pub pregame_games: u32,
    pub pregame_won: u32,
    pub pregame_lost: u32,
    /// Imported postboard games where only cards seen in the game log count
    /// as sided in (the SB In marker, spec ruling 8).
    pub inferred_games: u32,
}

/// One opponent archetype for the filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeOptionView {
    pub id: u32,
    pub name: String,
    pub color_identity: Option<String>,
}

/// The deck's game win rate under the filters (0.x `DeckWinrateData`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckWinrateView {
    pub wins: u32,
    pub games: u32,
    pub rate: f64,
}

/// The tab's data (0.x `cardStats` without `source`, `refreshedAt` and
/// `externalError`, spec ruling 10).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardStatsPayload {
    pub stats: Vec<CardStatView>,
    pub archetypes: Vec<ArchetypeOptionView>,
    pub perspective: Perspective,
    pub deck_winrate: DeckWinrateView,
    /// The `card_stats_trust` setting.
    pub trust: u32,
}

/// A store row as the UI receives it.
pub fn stat_view(stat: CardStat) -> CardStatView {
    CardStatView {
        key: stat.key,
        oracle_id: stat.oracle_id,
        catalog_id: stat.catalog_id,
        name: stat.name,
        color_identity: stat.color_identity,
        card_type: stat.card_type,
        image: stat.image,
        local_image: stat.local_image,
        is_sideboard: stat.is_sideboard,
        total_games: stat.total_games,
        total_possible: stat.total_possible,
        total_kept: stat.total_kept,
        kept_games: stat.kept_games,
        kept_won: stat.kept_won,
        kept_lost: stat.kept_lost,
        total_seen: stat.total_seen,
        seen_games: stat.seen_games,
        seen_won: stat.seen_won,
        seen_lost: stat.seen_lost,
        total_cast: stat.total_cast,
        cast_games: stat.cast_games,
        cast_won: stat.cast_won,
        cast_lost: stat.cast_lost,
        postboard_games: stat.postboard_games,
        sided_out_games: stat.sided_out_games,
        sided_in_games: stat.sided_in_games,
        total_played: stat.total_played,
        played_games: stat.played_games,
        total_kicked: stat.total_kicked,
        total_activated: stat.total_activated,
        total_flashback: stat.total_flashback,
        total_madness: stat.total_madness,
        total_evoked: stat.total_evoked,
        total_warp: stat.total_warp,
        total_free_cast: stat.total_free_cast,
        total_bargained: stat.total_bargained,
        total_dashed: stat.total_dashed,
        total_bestowed: stat.total_bestowed,
        total_replicated: stat.total_replicated,
        total_spectacle: stat.total_spectacle,
        total_rebound: stat.total_rebound,
        total_escaped: stat.total_escaped,
        total_ninjutsu: stat.total_ninjutsu,
        total_suspended: stat.total_suspended,
        total_buyback: stat.total_buyback,
        total_disturb: stat.total_disturb,
        total_foretold: stat.total_foretold,
        total_retraced: stat.total_retraced,
        total_mayhem: stat.total_mayhem,
        total_miracle: stat.total_miracle,
        total_gifted: stat.total_gifted,
        total_casualty: stat.total_casualty,
        pregame_revealed_games: stat.pregame_revealed_games,
        pregame_played_games: stat.pregame_played_games,
        pregame_games: stat.pregame_games,
        pregame_won: stat.pregame_won,
        pregame_lost: stat.pregame_lost,
        inferred_games: stat.inferred_games,
    }
}

/// The store filter for `query`.
pub fn stats_filter(
    version_id: Option<i64>,
    query: &CardStatsQuery,
    now_ms: i64,
    tz: Tz,
) -> CardStatsFilter {
    let (range, _previous) = timeframe_ranges(&query.timeframe, now_ms, tz);
    CardStatsFilter {
        version_id,
        timeframe: range,
        board: query.board.map(|board| match board {
            BoardKey::Game1 => Board::Game1,
            BoardKey::Postboard => Board::Postboard,
        }),
        on_play: query.play_draw.map(|side| side == PlayDrawKey::Play),
        opponent_archetype_id: query.archetype.map(i64::from),
        side: match query.perspective {
            Perspective::Mine => Side::Local,
            Perspective::Theirs => Side::Opponent,
        },
        best_of: query.best_of.map(store::BestOf::from),
    }
}

/// The tab's data for deck `deck_id`.
pub fn payload(
    conn: &Connection,
    deck_id: i64,
    version_id: Option<i64>,
    query: &CardStatsQuery,
    trust: u32,
    now_ms: i64,
    tz: Tz,
) -> Result<CardStatsPayload, CommandError> {
    let mut filter = stats_filter(version_id, query, now_ms, tz);
    filter.best_of = deck_best_of(conn, deck_id, query.best_of)?;
    let stats = card_stats(conn, deck_id, &filter).map_err(storage)?;
    let prior: DeckWinrate = deck_prior(conn, deck_id, &filter).map_err(storage)?;
    let archetypes: Vec<ArchetypeOption> =
        opponent_archetypes(conn, deck_id, version_id, filter.best_of).map_err(storage)?;
    Ok(payload_view(stats, prior, archetypes, query, trust))
}

/// The deck index's archetype Card stats tab: the same data over every
/// version of the archetype's decks (0.x `Decks\Archetypes\
/// CardStatsController`), with the opponent archetypes those versions
/// faced as the filter's options. The kind filter applies on the platform
/// the pages read, never on MTGO (its matches hold MTGO's structure text).
pub fn versions_payload(
    conn: &Connection,
    version_ids: &[i64],
    query: &CardStatsQuery,
    trust: u32,
    now_ms: i64,
    tz: Tz,
) -> Result<CardStatsPayload, CommandError> {
    let mut filter = stats_filter(None, query, now_ms, tz);
    let platform = store::platform::current_platform(conn).map_err(storage)?;
    filter.best_of = platform_best_of(platform, query.best_of);
    let stats =
        store::card_stats::card_stats_of_versions(conn, version_ids, &filter).map_err(storage)?;
    let prior =
        store::card_stats::deck_prior_of_versions(conn, version_ids, &filter).map_err(storage)?;
    let archetypes =
        store::card_stats::opponent_archetypes_of_versions(conn, version_ids, filter.best_of)
            .map_err(storage)?;
    Ok(payload_view(stats, prior, archetypes, query, trust))
}

fn payload_view(
    stats: Vec<CardStat>,
    prior: DeckWinrate,
    archetypes: Vec<ArchetypeOption>,
    query: &CardStatsQuery,
    trust: u32,
) -> CardStatsPayload {
    CardStatsPayload {
        stats: stats.into_iter().map(stat_view).collect(),
        archetypes: archetypes
            .into_iter()
            .map(|option| ArchetypeOptionView {
                id: u32::try_from(option.id).unwrap_or(0),
                name: option.name,
                color_identity: option.color_identity,
            })
            .collect(),
        perspective: query.perspective,
        deck_winrate: DeckWinrateView {
            wins: prior.wins,
            games: prior.games,
            rate: prior.rate,
        },
        trust,
    }
}

/// "Regenerate": refused for an archived deck; else the number of matches
/// the tracker regenerates.
pub fn regenerate(state: &AppState, deck_id: i64) -> Result<u32, CommandError> {
    let deck = store::decks::deck_by_id(&lock(&state.reader), deck_id).map_err(storage)?;
    if deck.is_some_and(|deck| deck.archived_at.is_some()) {
        return Err(CommandError::Refused(
            store::decks::ARCHIVED_DECK.to_string(),
        ));
    }
    let editor = state.tracker_part(tracker::TrackerHandle::deck_editor)?;
    editor
        .regenerate_card_stats(deck_id)
        .map_err(CommandError::Tracker)
}

/// Save the trust threshold, clamped to 0 to 100,000 (0.x
/// `UpdateTrustSettingController`), and return the value stored.
pub fn save_trust(state: &AppState, value: u32) -> Result<u32, CommandError> {
    let _one_save = lock(&state.settings_lock);
    let clamped = value.min(MAX_CARD_STATS_TRUST);
    state
        .save_setting_rows(vec![(CARD_STATS_TRUST.to_string(), clamped.to_string())])
        .map_err(CommandError::Tracker)?;
    Ok(read_settings(state)?.card_stats_trust)
}

#[tauri::command]
#[specta::specta]
pub fn deck_card_stats(
    deck_id: u32,
    version_id: Option<u32>,
    filters: CardStatsQuery,
    startup: State<'_, Startup>,
) -> Result<CardStatsPayload, CommandError> {
    let _timer = Timer::start("deck_card_stats");
    let state = ready(&startup)?;
    let trust = read_settings(state)?.card_stats_trust;
    payload(
        &lock(&state.reader),
        i64::from(deck_id),
        version_id.map(i64::from),
        &filters,
        trust,
        crate::wall_clock_ms(),
        state.timezone,
    )
}

/// Async so the wait for the tracker never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn regenerate_card_stats(app: AppHandle, deck_id: u32) -> Result<u32, CommandError> {
    let _timer = Timer::start("regenerate_card_stats");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        regenerate(ready(&startup)?, i64::from(deck_id))
    })
    .await
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn set_card_stats_trust(app: AppHandle, value: u32) -> Result<u32, CommandError> {
    let _timer = Timer::start("set_card_stats_trust");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let stored = save_trust(state, value)?;
        let settings = read_settings(state)?;
        if let Err(error) = (SettingsChanged { settings }).emit(&app) {
            tracing::warn!(%error, "could not push the settings change");
        }
        Ok(stored)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;

    const NOW: i64 = 1_791_374_400_000;

    fn query() -> CardStatsQuery {
        CardStatsQuery {
            timeframe: "alltime".to_string(),
            board: None,
            play_draw: None,
            archetype: None,
            perspective: Perspective::Mine,
            best_of: None,
        }
    }

    /// Wait until the tracker's startup game backfill has run (S-F1): the
    /// backfill syncs every log match from its events, so a match seeded
    /// while its list was being read lost its card rows (it has no events).
    fn settled(state: &AppState) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let version = store::game_sync::game_cards_version(
                &store::open_reader(&state.db_path).expect("reader"),
            )
            .expect("version");
            if version.is_some_and(|version| version >= store::game_sync::GAME_CARDS_VERSION) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the tracker's game backfill never finished"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Deck 1 with one version, one complete log match of two games (a win
    /// on the play, a loss on the draw) and one card in both; returns the
    /// deck id. Written on a writer connection once the tracker settled, as
    /// the tracker would.
    fn seed(state: &AppState, archived: bool) -> i64 {
        settled(state);
        let conn = store::open_writer(&state.db_path).expect("writer");
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at,
                 source, archived_at)
                 VALUES (1, 'q-deck', 1, 'Qdeck', 'CMODERN', 0, 0, 'file', {archived});
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (1, 1, 'Q', 0);
             INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
                 VALUES (77, 'q-oracle', 'Qbolt', 'Instant', 'R');
             INSERT INTO matches (id, token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version, deck_version_id)
                 VALUES (1, 'q-match', 1, 'log', 'complete', 'win', {NOW}, {NOW}, 1, 1, 1);
             INSERT INTO games (id, match_id, game_number, result, end_reason, on_play)
                 VALUES (1, 1, 1, 'win', 'win', 1), (2, 1, 2, 'loss', 'win', 0);
             INSERT INTO game_cards (game_id, side, catalog_id, main_qty, cast, inferred)
                 VALUES (1, 'local', 77, 4, 1, 0), (2, 'local', 77, 4, 0, 1);
             INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
                 VALUES (1, 'local', 77, 'free_cast', 2);",
            archived = if archived { "1" } else { "NULL" },
        ))
        .expect("seed");
        1
    }

    // Endstep plan 3: an Endstep deck's card stats come from its stored game
    // rows, as any other deck's.
    #[test]
    fn card_stats_count_an_endstep_deck() {
        let dir = TestDir::new("card-stats-endstep");
        let path = dir.join("mymtgo.sqlite");
        let now = 1_794_400_000_000;
        crate::endstep_seed::sync_endstep_fixture(&path, &dir, now);
        let conn = store::open_reader(&path).expect("reader");
        let deck_id: i64 = conn
            .query_row(
                "SELECT v.deck_id FROM matches m
                 JOIN deck_versions v ON v.id = m.deck_version_id",
                [],
                |row| row.get(0),
            )
            .expect("the match's deck");
        let found = payload(&conn, deck_id, None, &query(), 75, now, Tz::UTC).expect("payload");
        let map = found
            .stats
            .iter()
            .find(|stat| stat.name == "Expedition Map")
            .expect("Expedition Map");
        // Cast once in game 1 and once in a later game (the fixture's two
        // `Player cast Expedition Map` events).
        let cast_in: Vec<i64> = conn
            .prepare(
                "SELECT g.game_number FROM game_cards c JOIN games g ON g.id = c.game_id
                 WHERE c.side = 'local' AND c.catalog_id = ?1 AND c.cast > 0
                 ORDER BY g.game_number",
            )
            .expect("sql")
            .query_map([map.catalog_id], |row| row.get(0))
            .expect("rows")
            .collect::<Result<_, _>>()
            .expect("numbers");
        assert_eq!(cast_in.len(), 2, "{cast_in:?}");
        assert_eq!(cast_in[0], 1);
        assert_eq!(map.cast_games, 2);
        assert_eq!(map.total_games, 3);
        assert_eq!(found.deck_winrate.games, 3);
    }

    #[test]
    fn the_payload_has_0xs_shape_without_the_community_parts() {
        let dir = TestDir::new("card-stats-payload");
        let (state, _events) = ready_state(&dir);
        let deck_id = seed(&state, false);
        let found = payload(
            &lock(&state.reader),
            deck_id,
            None,
            &query(),
            75,
            NOW,
            Tz::UTC,
        )
        .expect("payload");
        assert_eq!(found.trust, 75);
        assert_eq!(found.stats.len(), 1);
        let stat = &found.stats[0];
        assert_eq!(
            (stat.total_games, stat.cast_games, stat.cast_won),
            (2, 1, 1)
        );
        assert_eq!((stat.total_free_cast, stat.inferred_games), (2, 1));
        assert_eq!((found.deck_winrate.wins, found.deck_winrate.games), (1, 2));

        let json = serde_json::to_value(&found).expect("json");
        let keys: Vec<&String> = json.as_object().expect("object").keys().collect();
        assert_eq!(
            keys,
            ["archetypes", "deckWinrate", "perspective", "stats", "trust"]
        );
        assert_eq!(json["perspective"], "mine");
        assert_eq!(json["deckWinrate"]["rate"], 0.5);
        let row = &json["stats"][0];
        assert_eq!(row["type"], "Instant");
        assert_eq!(row["oracleId"], "q-oracle");
        assert_eq!(row["colorIdentity"], "R");
        assert_eq!(row["isSideboard"], false);
        assert_eq!(row["totalFreeCast"], 2);
        assert_eq!(row["inferredGames"], 1);
        assert_eq!(row["pregameLost"], 0);
        assert!(row.get("source").is_none());

        // The filters reach the store: the draw game only.
        let draw = CardStatsQuery {
            play_draw: Some(PlayDrawKey::Draw),
            board: Some(BoardKey::Postboard),
            ..query()
        };
        let found =
            payload(&lock(&state.reader), deck_id, None, &draw, 75, NOW, Tz::UTC).expect("draw");
        assert_eq!(found.stats[0].total_games, 1);
        assert_eq!(found.deck_winrate.rate, 0.0);
        // A timeframe the match is not in.
        let old = CardStatsQuery {
            timeframe: "2020-01-01..2020-01-31".to_string(),
            ..query()
        };
        let found =
            payload(&lock(&state.reader), deck_id, None, &old, 75, NOW, Tz::UTC).expect("old");
        assert!(found.stats.is_empty());
        let theirs = CardStatsQuery {
            perspective: Perspective::Theirs,
            ..query()
        };
        let found = payload(
            &lock(&state.reader),
            deck_id,
            None,
            &theirs,
            75,
            NOW,
            Tz::UTC,
        )
        .expect("theirs");
        assert_eq!(found.perspective, Perspective::Theirs);
        assert!(found.stats.is_empty());
    }

    /// Both tabs offer the opponent archetypes faced (0.x `archetypes`
    /// prop: id, name, colour identity).
    #[test]
    fn both_tabs_offer_the_opponent_archetypes_faced() {
        let dir = TestDir::new("card-stats-archetype-options");
        let (state, _events) = ready_state(&dir);
        let deck_id = seed(&state, false);
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute_batch(
                "INSERT INTO archetypes (id, uuid, name, format, color_identity, source,
                     created_at, updated_at)
                     VALUES (50, 'q-uuid', 'Qburn', 'modern', 'R', 'manual', 0, 0);
                 INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                     VALUES (1, 1, 50, 1.0);",
            )
            .expect("seat");
        let deck = payload(
            &lock(&state.reader),
            deck_id,
            None,
            &query(),
            75,
            NOW,
            Tz::UTC,
        )
        .expect("deck tab");
        let expected = vec![ArchetypeOptionView {
            id: 50,
            name: "Qburn".to_string(),
            color_identity: Some("R".to_string()),
        }];
        assert_eq!(deck.archetypes, expected);
        let tab = versions_payload(&lock(&state.reader), &[1], &query(), 75, NOW, Tz::UTC)
            .expect("archetype tab");
        assert_eq!(tab.archetypes, expected);
        // MTGO matches have no Bo1/Bo3 kind: a stray kind filters nothing.
        let bo1 = CardStatsQuery {
            best_of: Some(BestOfView::Bo1),
            ..query()
        };
        let stray = versions_payload(&lock(&state.reader), &[1], &bo1, 75, NOW, Tz::UTC)
            .expect("archetype tab");
        assert_eq!(stray, tab);
    }

    #[test]
    fn the_filters_read_0xs_values() {
        let parsed: CardStatsQuery = serde_json::from_value(serde_json::json!({
            "timeframe": "week",
            "board": "postboard",
            "playDraw": "play",
            "archetype": 4,
            "perspective": "theirs",
        }))
        .expect("query");
        let filter = stats_filter(Some(3), &parsed, NOW, Tz::UTC);
        assert_eq!(filter.version_id, Some(3));
        assert_eq!(filter.board, Some(Board::Postboard));
        assert_eq!(filter.on_play, Some(true));
        assert_eq!(filter.opponent_archetype_id, Some(4));
        assert_eq!(filter.side, Side::Opponent);
        assert!(filter.timeframe.from_ms > 0, "the week is applied");
    }

    #[test]
    fn trust_is_clamped() {
        let dir = TestDir::new("card-stats-trust");
        let (state, _events) = ready_state(&dir);
        assert_eq!(save_trust(&state, 200_000).expect("save"), 100_000);
        assert_eq!(
            read_settings(&state).expect("read").card_stats_trust,
            100_000
        );
        assert_eq!(save_trust(&state, 0).expect("save"), 0);
        assert_eq!(save_trust(&state, 120).expect("save"), 120);
        assert_eq!(read_settings(&state).expect("read").card_stats_trust, 120);
    }

    #[test]
    fn an_archived_deck_is_not_regenerated() {
        let dir = TestDir::new("card-stats-regenerate-archived");
        let (state, _events) = ready_state(&dir);
        let deck_id = seed(&state, true);
        let refused = regenerate(&state, deck_id).expect_err("refused");
        assert_eq!(
            refused,
            CommandError::Refused(store::decks::ARCHIVED_DECK.to_string())
        );
    }

    #[test]
    fn regenerate_answers_the_matches_regenerated() {
        let dir = TestDir::new("card-stats-regenerate");
        let (state, _events) = ready_state(&dir);
        let deck_id = seed(&state, false);
        assert_eq!(regenerate(&state, deck_id).expect("regenerated"), 1);
    }
}
