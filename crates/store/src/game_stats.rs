//! The deck Game Stats page (archetypes spec section 12, 0.x
//! `AggregateGameStats` and `GameStatsController`): wins, losses, win rate
//! and averages for every game and for games 1 to 3, overall, on the play
//! and on the draw, optionally against one opponent archetype (by uuid).

use decks::php_round;
use decks::stats::Range;
use rusqlite::{Connection, params};

use crate::StoreError;

/// Which games a row counts (0.x `group`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameGroup {
    AllGames,
    Game1,
    Game2,
    Game3,
}

impl GameGroup {
    /// Every group, in 0.x's row order.
    pub const ALL: [GameGroup; 4] = [
        GameGroup::AllGames,
        GameGroup::Game1,
        GameGroup::Game2,
        GameGroup::Game3,
    ];

    /// 0.x's key: `all_games`, `game_1`, `game_2`, `game_3`.
    pub fn key(self) -> &'static str {
        match self {
            GameGroup::AllGames => "all_games",
            GameGroup::Game1 => "game_1",
            GameGroup::Game2 => "game_2",
            GameGroup::Game3 => "game_3",
        }
    }

    /// The game number the group keeps; `None` keeps every game.
    fn number(self) -> Option<u32> {
        match self {
            GameGroup::AllGames => None,
            GameGroup::Game1 => Some(1),
            GameGroup::Game2 => Some(2),
            GameGroup::Game3 => Some(3),
        }
    }
}

/// Which side a row counts (0.x `split`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaySplit {
    Overall,
    Play,
    Draw,
}

impl PlaySplit {
    pub const ALL: [PlaySplit; 3] = [PlaySplit::Overall, PlaySplit::Play, PlaySplit::Draw];

    /// 0.x's key: `overall`, `play`, `draw`.
    pub fn key(self) -> &'static str {
        match self {
            PlaySplit::Overall => "overall",
            PlaySplit::Play => "play",
            PlaySplit::Draw => "draw",
        }
    }
}

/// One row of the Game Stats table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameStatsRow {
    pub group: GameGroup,
    pub split: PlaySplit,
    pub wins: u32,
    pub losses: u32,
    /// Wins over decided games, percent with one decimal; `None` without
    /// games.
    pub win_rate: Option<f64>,
    /// Averages per game over the games that know them, two decimals.
    pub mulligans: Option<f64>,
    pub opponent_mulligans: Option<f64>,
    pub turns: Option<f64>,
}

/// One game read for the table.
struct StatGame {
    match_id: i64,
    won: bool,
    on_play: bool,
    turns: Option<i64>,
    mulligans: Option<i64>,
    opponent_mulligans: Option<i64>,
}

/// The won or lost games of deck `?1`'s listed complete matches started
/// from `?2` to `?3`, against the opponent archetype with uuid `?4` (any
/// when NULL), in play order per match.
const GAMES: &str = "
SELECT g.match_id, g.result = 'win', COALESCE(g.on_play, 0), g.turn_count,
       g.local_mulligans, g.opp_mulligans
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
JOIN games g ON g.match_id = m.id
WHERE v.deck_id = ?1 AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
  AND g.result IN ('win', 'loss')
  AND (?4 IS NULL OR EXISTS (
        SELECT 1 FROM match_archetypes ma JOIN archetypes a ON a.id = ma.archetype_id
        WHERE ma.match_id = m.id AND ma.is_opponent = 1 AND a.uuid = ?4))
ORDER BY g.match_id, g.started_at, g.id";

/// The average of the known values, two decimals; `None` without any.
fn average(values: impl Iterator<Item = Option<i64>>) -> Option<f64> {
    let known: Vec<i64> = values.flatten().collect();
    if known.is_empty() {
        return None;
    }
    let sum: i64 = known.iter().sum();
    Some(php_round(sum as f64 / known.len() as f64, 2))
}

/// The twelve Game Stats rows of deck `deck_id` in `range` (0.x
/// `AggregateGameStats::run`): games numbered within their match among the
/// won or lost ones; a game with no play side counts as on the draw (0.x
/// casts it to false).
pub fn game_stats(
    conn: &Connection,
    deck_id: i64,
    range: Range,
    opponent_uuid: Option<&str>,
) -> Result<Vec<GameStatsRow>, StoreError> {
    let mut stmt = conn.prepare_cached(GAMES)?;
    let rows = stmt.query_map(
        params![deck_id, range.from_ms, range.to_ms, opponent_uuid],
        |row| {
            Ok(StatGame {
                match_id: row.get(0)?,
                won: row.get(1)?,
                on_play: row.get(2)?,
                turns: row.get(3)?,
                mulligans: row.get(4)?,
                opponent_mulligans: row.get(5)?,
            })
        },
    )?;
    let mut games: Vec<(u32, StatGame)> = Vec::new();
    let mut last_match = None;
    let mut number = 0;
    for row in rows {
        let game = row?;
        if last_match != Some(game.match_id) {
            last_match = Some(game.match_id);
            number = 0;
        }
        number += 1;
        games.push((number, game));
    }

    let mut out = Vec::new();
    for group in GameGroup::ALL {
        for split in PlaySplit::ALL {
            let scoped: Vec<&StatGame> = games
                .iter()
                .filter(|(number, game)| {
                    group.number().is_none_or(|wanted| wanted == *number)
                        && match split {
                            PlaySplit::Overall => true,
                            PlaySplit::Play => game.on_play,
                            PlaySplit::Draw => !game.on_play,
                        }
                })
                .map(|(_, game)| game)
                .collect();
            let wins = scoped.iter().filter(|game| game.won).count() as u32;
            let losses = scoped.len() as u32 - wins;
            let decided = wins + losses;
            out.push(GameStatsRow {
                group,
                split,
                wins,
                losses,
                win_rate: if decided > 0 {
                    Some(php_round(f64::from(wins) / f64::from(decided) * 100.0, 1))
                } else {
                    None
                },
                mulligans: average(scoped.iter().map(|game| game.mulligans)),
                opponent_mulligans: average(scoped.iter().map(|game| game.opponent_mulligans)),
                turns: average(scoped.iter().map(|game| game.turns)),
            });
        }
    }
    Ok(out)
}

/// An opponent archetype the Game Stats filter offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpponentOption {
    pub uuid: String,
    pub name: String,
    /// Canonical comma form.
    pub color_identity: Option<String>,
}

/// The opponent archetypes deck `deck_id` faced in a listed complete match,
/// by name (0.x `GameStatsController::opponentOptions`).
pub fn game_stats_opponents(
    conn: &Connection,
    deck_id: i64,
) -> Result<Vec<OpponentOption>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT a.uuid, a.name, a.color_identity
         FROM deck_versions v
         JOIN listed_matches m ON m.deck_version_id = v.id
         JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
         JOIN archetypes a ON a.id = ma.archetype_id
         WHERE v.deck_id = ?1 AND m.state = 'complete'
         ORDER BY a.name, a.uuid",
    )?;
    let rows = stmt.query_map([deck_id], |row| {
        let colors: Option<String> = row.get(2)?;
        Ok(OpponentOption {
            uuid: row.get(0)?,
            name: row.get(1)?,
            color_identity: archetypes::normalize_colors(colors.as_deref()),
        })
    })?;
    let mut options = Vec::new();
    for row in rows {
        options.push(row?);
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matchups::seed::*;
    use decks::stats::Range;

    const DAY: i64 = 86_400_000;
    /// 2026-03-15T12:00:00Z.
    const MARCH_15: i64 = 1_773_576_000_000;

    fn row(rows: &[GameStatsRow], group: GameGroup, split: PlaySplit) -> GameStatsRow {
        rows.iter()
            .find(|row| row.group == group && row.split == split)
            .copied()
            .expect("row")
    }

    fn wins_losses(row: GameStatsRow) -> (u32, u32) {
        (row.wins, row.losses)
    }

    fn all(conn: &Connection, deck: i64) -> Vec<GameStatsRow> {
        game_stats(conn, deck, Range::all_time(), None).expect("stats")
    }

    // 0.x AggregateGameStatsTest "produces a 12-row breakdown grouped by
    // game number and on-play split".
    #[test]
    fn there_are_twelve_rows_by_game_and_play_side() {
        let (_dir, conn) = db("game-stats-rows");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("win", false)],
        );

        let rows = all(&conn, deck);
        assert_eq!(rows.len(), 12);
        let order: Vec<(GameGroup, PlaySplit)> =
            rows.iter().map(|row| (row.group, row.split)).collect();
        assert_eq!(order[0], (GameGroup::AllGames, PlaySplit::Overall));
        assert_eq!(order[1], (GameGroup::AllGames, PlaySplit::Play));
        assert_eq!(order[11], (GameGroup::Game3, PlaySplit::Draw));
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Overall)),
            (2, 0)
        );
        assert_eq!(
            wins_losses(row(&rows, GameGroup::Game1, PlaySplit::Overall)),
            (1, 0)
        );
        assert_eq!(
            wins_losses(row(&rows, GameGroup::Game2, PlaySplit::Overall)),
            (1, 0)
        );
        assert_eq!(
            wins_losses(row(&rows, GameGroup::Game3, PlaySplit::Overall)),
            (0, 0)
        );
        assert_eq!(GameGroup::AllGames.key(), "all_games");
        assert_eq!(PlaySplit::Draw.key(), "draw");
    }

    // "partitions wins and losses by on-play vs on-draw".
    #[test]
    fn games_split_on_the_play_and_on_the_draw() {
        let (_dir, conn) = db("game-stats-split");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("loss", false)],
        );
        against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15,
            &[g("win", false), g("loss", true)],
        );

        let rows = all(&conn, deck);
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Play)),
            (1, 1)
        );
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Draw)),
            (1, 1)
        );
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Overall)),
            (2, 2)
        );
    }

    // "averages local and opponent mulligans per game".
    #[test]
    fn mulligans_are_averaged_per_game() {
        let (_dir, conn) = db("game-stats-mulligans");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let game = |result, on_play, mine, theirs| Game {
            mulligans: Some(mine),
            opponent_mulligans: Some(theirs),
            ..g(result, on_play)
        };
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[game("win", true, 0, 1), game("win", false, 1, 0)],
        );
        against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15,
            &[game("loss", true, 2, 2), game("loss", false, 1, 1)],
        );

        let overall = row(&all(&conn, deck), GameGroup::AllGames, PlaySplit::Overall);
        assert_eq!(overall.mulligans, Some(1.0));
        assert_eq!(overall.opponent_mulligans, Some(1.0));
    }

    // "averages turns excluding null turn counts but still counts wins and
    // losses".
    #[test]
    fn turns_are_averaged_over_games_that_know_them() {
        let (_dir, conn) = db("game-stats-turns");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let game = |on_play, turns| Game {
            turns,
            ..g("win", on_play)
        };
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[game(true, Some(7)), game(false, None)],
        );
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[game(true, Some(11))],
        );

        let overall = row(&all(&conn, deck), GameGroup::AllGames, PlaySplit::Overall);
        assert_eq!(wins_losses(overall), (3, 0));
        assert_eq!(overall.turns, Some(9.0));
    }

    // "returns null averages and win rate when no games match a bucket".
    #[test]
    fn an_empty_bucket_has_no_rate_or_averages() {
        let (_dir, conn) = db("game-stats-empty");
        let deck = deck(&conn, None, "CMODERN", None);
        version(&conn, deck);
        let overall = row(&all(&conn, deck), GameGroup::AllGames, PlaySplit::Overall);
        assert_eq!(
            overall,
            GameStatsRow {
                group: GameGroup::AllGames,
                split: PlaySplit::Overall,
                wins: 0,
                losses: 0,
                win_rate: None,
                mulligans: None,
                opponent_mulligans: None,
                turns: None,
            }
        );
    }

    // "computes win_rate as a percentage with one decimal".
    #[test]
    fn the_win_rate_has_one_decimal() {
        let (_dir, conn) = db("game-stats-rate");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("win", false), g("loss", true)],
        );
        let overall = row(&all(&conn, deck), GameGroup::AllGames, PlaySplit::Overall);
        assert_eq!(overall.win_rate, Some(66.7));
    }

    // "excludes matches outside the requested timeframe" and GameStatsController
    // "respects the timeframe query parameter".
    #[test]
    fn only_matches_in_the_window_count() {
        let (_dir, conn) = db("game-stats-window");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15 - 2 * DAY,
            &[g("win", true)],
        );
        against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15 - 180 * DAY,
            &[g("loss", false)],
        );

        let week = Range {
            from_ms: MARCH_15 - 7 * DAY,
            to_ms: MARCH_15,
        };
        let rows = game_stats(&conn, deck, week, None).expect("stats");
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Overall)),
            (1, 0)
        );
    }

    // "filters by opponent archetype uuid when provided" and the controller's
    // "respects the opponent query parameter".
    #[test]
    fn an_opponent_uuid_keeps_the_matches_against_it() {
        let (_dir, conn) = db("game-stats-opponent");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let qtron = archetype(&conn, "Qtron", "modern", None);
        against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        against(&conn, version, qtron, "loss", MARCH_15, &[g("loss", false)]);
        // The player seat never counts as the opponent.
        let mirror = against(&conn, version, qtron, "loss", MARCH_15, &[g("loss", false)]);
        player_seat(&conn, mirror, qburn);

        let uuid = uuid_of(&conn, qburn);
        let rows = game_stats(&conn, deck, Range::all_time(), Some(&uuid)).expect("stats");
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Overall)),
            (1, 0)
        );
    }

    // "includes games beyond game 3 in all_games but not in the game_1/2/3
    // rows".
    #[test]
    fn a_fourth_game_counts_only_in_all_games() {
        let (_dir, conn) = db("game-stats-fourth");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[
                g("win", true),
                g("loss", false),
                g("win", true),
                g("win", false),
            ],
        );
        let rows = all(&conn, deck);
        assert_eq!(
            wins_losses(row(&rows, GameGroup::AllGames, PlaySplit::Overall)),
            (3, 1)
        );
        assert_eq!(
            wins_losses(row(&rows, GameGroup::Game3, PlaySplit::Overall)),
            (1, 0)
        );
    }

    // GameStatsController "renders the game stats page with stats rows and
    // opponent options": the archetypes faced, by name.
    #[test]
    fn the_opponent_options_are_the_archetypes_faced() {
        let (_dir, conn) = db("game-stats-options");
        let deck = deck(&conn, None, "CMODERN", None);
        let other = version(
            &conn,
            crate::matchups::seed::deck(&conn, None, "CMODERN", None),
        );
        let version = version(&conn, deck);
        let qtron = archetype(&conn, "Qtron", "modern", Some("C"));
        let qburn = archetype(&conn, "Qburn", "modern", Some("R"));
        let unused = archetype(&conn, "Qunused", "modern", None);
        against(&conn, version, qtron, "win", MARCH_15, &[g("win", true)]);
        against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        against(&conn, other, unused, "win", MARCH_15, &[g("win", true)]);
        let mirror = complete(&conn, version, "win", MARCH_15);
        player_seat(&conn, mirror, unused);

        let options = game_stats_opponents(&conn, deck).expect("options");
        let names: Vec<&str> = options.iter().map(|option| option.name.as_str()).collect();
        assert_eq!(names, ["Qburn", "Qtron"]);
        assert_eq!(options[0].uuid, uuid_of(&conn, qburn));
        assert_eq!(options[0].color_identity.as_deref(), Some("R"));
    }
}
