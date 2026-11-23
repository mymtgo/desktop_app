//! Deck numbers (decks spec section 7): 0.x `MatchRecord`, `Winrate` and
//! `SampleRate`,
//! `TimeframeRange`, `GetBoardingSplit`'s rates, `GetDailyMatchResults` with
//! the dashboard's dense chart days and sparkline, and the version date
//! labels of `GetDeckVersionStats`.
//!
//! Pure: callers pass in `now`, the timezone and the rows read from the
//! database.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use mtgo_match::Outcome;

/// A win rate as a whole percentage: wins over every match played, draws
/// included. 0.x `Winrate::percentage`, `round(100 * $wins / $total)`: the
/// multiply comes first, so 23 of 40 is 58 (compare
/// [`sample_rate_percent`]). 0 when nothing was played. Game rates pass no
/// draws.
pub fn winrate_percent(wins: u32, losses: u32, draws: u32) -> u32 {
    let total = u64::from(wins) + u64::from(losses) + u64::from(draws);
    if total == 0 {
        return 0;
    }
    let rate = 100.0 * f64::from(wins) / total as f64;
    crate::php_round(rate, 0) as u32
}

/// A count as a whole percentage of its sample, `None` without a sample.
/// 0.x `SampleRate::percentage`, `round($count / $games * 100)`: the divide
/// comes first, so 23 of 40 is 57 (0.575 is just under in binary; compare
/// [`winrate_percent`]).
pub fn sample_rate_percent(count: u32, games: u32) -> Option<u32> {
    if games == 0 {
        return None;
    }
    let rate = f64::from(count) / f64::from(games) * 100.0;
    Some(crate::php_round(rate, 0) as u32)
}

/// Wins, losses and draws of a set of matches (0.x `MatchRecord`). Anything
/// that is neither a win nor a loss (a draw, an unknown outcome) is a draw,
/// so the rate always agrees with the record shown beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Record {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    /// `wins + losses + draws`.
    pub total: u32,
    /// `winrate_percent(wins, losses, draws)`.
    pub win_rate: u32,
}

impl Record {
    pub fn from_counts(wins: u32, losses: u32, draws: u32) -> Record {
        Record {
            wins,
            losses,
            draws,
            total: wins + losses + draws,
            win_rate: winrate_percent(wins, losses, draws),
        }
    }

    /// From a count of every match and its wins and losses; the rest are
    /// draws, never fewer than none.
    pub fn from_total(wins: u32, losses: u32, total: u32) -> Record {
        let draws = total.saturating_sub(wins + losses);
        Record::from_counts(wins, losses, draws)
    }

    pub fn plus(self, other: Record) -> Record {
        Record::from_counts(
            self.wins + other.wins,
            self.losses + other.losses,
            self.draws + other.draws,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// `W - L`, or `W - L - D` when there are draws (0.x `MatchRecord::label`).
pub fn record_label(record: &Record) -> String {
    if record.draws > 0 {
        format!("{} - {} - {}", record.wins, record.losses, record.draws)
    } else {
        format!("{} - {}", record.wins, record.losses)
    }
}

/// Games won and lost (games with a known result only) and their rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GameRecord {
    pub won: u32,
    pub lost: u32,
    pub win_rate: u32,
}

impl GameRecord {
    pub fn new(won: u32, lost: u32) -> GameRecord {
        GameRecord {
            won,
            lost,
            win_rate: winrate_percent(won, lost, 0),
        }
    }

    pub fn plus(self, other: GameRecord) -> GameRecord {
        GameRecord::new(self.won + other.won, self.lost + other.lost)
    }
}

/// Games on the play and on the draw (0.x `otpWon` ... `playDrawGames`).
/// `play_draw_games` counts every game that knows who was on the play, its
/// result known or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayDraw {
    pub otp_won: u32,
    pub otp_lost: u32,
    pub otp_rate: u32,
    pub otd_won: u32,
    pub otd_lost: u32,
    pub otd_rate: u32,
    pub play_draw_games: u32,
}

impl PlayDraw {
    pub fn new(
        otp_won: u32,
        otp_lost: u32,
        otd_won: u32,
        otd_lost: u32,
        play_draw_games: u32,
    ) -> PlayDraw {
        PlayDraw {
            otp_won,
            otp_lost,
            otp_rate: winrate_percent(otp_won, otp_lost, 0),
            otd_won,
            otd_lost,
            otd_rate: winrate_percent(otd_won, otd_lost, 0),
            play_draw_games,
        }
    }

    pub fn plus(self, other: PlayDraw) -> PlayDraw {
        PlayDraw::new(
            self.otp_won + other.otp_won,
            self.otp_lost + other.otp_lost,
            self.otd_won + other.otd_won,
            self.otd_lost + other.otd_lost,
            self.play_draw_games + other.play_draw_games,
        )
    }
}

/// Game one against the games after sideboarding (0.x `GetBoardingSplit`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BoardingSplit {
    pub game_one_won: u32,
    pub game_one_lost: u32,
    pub game_one_rate: u32,
    pub post_board_won: u32,
    pub post_board_lost: u32,
    pub post_board_rate: u32,
    /// `post_board_rate - game_one_rate`.
    pub delta: i32,
    /// Every game counted.
    pub games: u32,
}

impl BoardingSplit {
    pub fn new(
        game_one_won: u32,
        game_one_lost: u32,
        post_board_won: u32,
        post_board_lost: u32,
    ) -> BoardingSplit {
        let game_one_rate = winrate_percent(game_one_won, game_one_lost, 0);
        let post_board_rate = winrate_percent(post_board_won, post_board_lost, 0);
        BoardingSplit {
            game_one_won,
            game_one_lost,
            game_one_rate,
            post_board_won,
            post_board_lost,
            post_board_rate,
            delta: post_board_rate as i32 - game_one_rate as i32,
            games: game_one_won + game_one_lost + post_board_won + post_board_lost,
        }
    }
}

/// A window of time in UTC milliseconds, both ends included (0.x
/// `whereBetween`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub from_ms: i64,
    pub to_ms: i64,
}

impl Range {
    /// Every match ever played.
    pub fn all_time() -> Range {
        Range {
            from_ms: i64::MIN,
            to_ms: i64::MAX,
        }
    }
}

/// The window a timeframe key covers (0.x `TimeframeRange::run`), in whole
/// local days of `tz`: from local midnight of the first day to the last
/// millisecond of today (or of a custom range's last day).
///
/// Keys: `week` (today and the 7 days before it), `biweekly` (14 days
/// before), `monthly` (30 days before), `year` (since 1 January), and a
/// custom `YYYY-MM-DD..YYYY-MM-DD` (both days included, either order).
/// `alltime`, an unknown key and a malformed custom range give `None`: all
/// time, without bounds.
pub fn timeframe_range(key: &str, now_ms: i64, tz: Tz) -> Option<Range> {
    if let Some((first, last)) = custom_days(key) {
        return Some(Range {
            from_ms: local_midnight(first, tz),
            to_ms: end_of_day(last, tz),
        });
    }
    let today = local_day(now_ms, tz);
    let first = preset_start(key, today)?;
    Some(Range {
        from_ms: local_midnight(first, tz),
        to_ms: end_of_day(today, tz),
    })
}

/// The window just before the current one (0.x `TimeframeRange::previous`):
/// it ends 1 millisecond before `current_from_ms` and goes back the same
/// number of days, to local midnight. `None` when the key has no previous
/// window (all time, unknown or malformed keys).
pub fn previous_range(key: &str, current_from_ms: i64, tz: Tz) -> Option<Range> {
    let to_ms = current_from_ms - 1;
    let last = local_day(to_ms, tz);
    let first = match custom_days(key) {
        Some((custom_first, custom_last)) => {
            let days = (custom_last - custom_first).num_days();
            last - Duration::days(days)
        }
        None => preset_start(key, last)?,
    };
    Some(Range {
        from_ms: local_midnight(first, tz),
        to_ms,
    })
}

/// The window 0.x deck pages without a timeframe use for their version rows
/// (`GetDeckViewSharedProps`): from UTC midnight two months ago to the end of
/// today in UTC. Months are subtracted like Carbon's `subMonths`: a day past
/// the end of the target month spills into the next one (30 April gives
/// 2 March).
pub fn default_version_range(now_ms: i64) -> Range {
    let today = local_day(now_ms, chrono_tz::UTC);
    let first = carbon_sub_months(today, 2);
    Range {
        from_ms: local_midnight(first, chrono_tz::UTC),
        to_ms: end_of_day(today, chrono_tz::UTC),
    }
}

/// `day` minus `months` months the way Carbon (PHP `DateTime`) does it: the
/// same day number in the target month, with any days past that month's end
/// carried into the next month (31 January minus 2 months is 1 December).
fn carbon_sub_months(day: NaiveDate, months: u32) -> NaiveDate {
    let mut year = day.year();
    let mut month = day.month() as i32 - months as i32;
    while month < 1 {
        month += 12;
        year -= 1;
    }
    let Some(first_of_month) = NaiveDate::from_ymd_opt(year, month as u32, 1) else {
        return day;
    };
    first_of_month + Duration::days(i64::from(day.day()) - 1)
}

/// The first day of a preset window ending on `last`, `None` for a key
/// that is not a preset.
fn preset_start(key: &str, last: NaiveDate) -> Option<NaiveDate> {
    match key {
        "week" => Some(last - Duration::days(7)),
        "biweekly" => Some(last - Duration::days(14)),
        "monthly" => Some(last - Duration::days(30)),
        "year" => NaiveDate::from_ymd_opt(last.year(), 1, 1),
        _ => None,
    }
}

/// The two days of a custom `YYYY-MM-DD..YYYY-MM-DD` key, earliest first.
/// `None` for anything else: the key comes from the UI and is not trusted,
/// and a day that does not exist (`2026-02-31`) is refused.
fn custom_days(key: &str) -> Option<(NaiveDate, NaiveDate)> {
    let parts: Vec<&str> = key.split("..").collect();
    if parts.len() != 2 {
        return None;
    }
    let first = parse_day(parts[0])?;
    let second = parse_day(parts[1])?;
    if first <= second {
        Some((first, second))
    } else {
        Some((second, first))
    }
}

/// A `YYYY-MM-DD` day, exactly that shape.
fn parse_day(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = if index == 4 || index == 7 {
            *byte == b'-'
        } else {
            byte.is_ascii_digit()
        };
        if !ok {
            return None;
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// The local day of a UTC instant.
fn local_day(ms: i64, tz: Tz) -> NaiveDate {
    let instant = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    instant.with_timezone(&tz).date_naive()
}

/// UTC milliseconds of the first moment of a local day (0.x `startOfDay()`).
/// Midnight can fall in a clock change gap in some zones; it then moves on
/// by the gap, as PHP moves a time in a gap forward.
pub fn local_midnight(day: NaiveDate, tz: Tz) -> i64 {
    let midnight = day.and_hms_opt(0, 0, 0).unwrap_or_default();
    mtgo_protocol::local_to_utc_ms(midnight, tz)
        .unwrap_or_else(|| midnight.and_utc().timestamp_millis())
}

/// UTC milliseconds of the last millisecond of a local day.
pub fn end_of_day(day: NaiveDate, tz: Tz) -> i64 {
    local_midnight(day + Duration::days(1), tz) - 1
}

/// The whole local days from `day` (`YYYY-MM-DD`) to the local day of
/// `now_ms`: 0 for today, for a day ahead or for text that is not a day.
pub fn days_ago(day: &str, now_ms: i64, tz: Tz) -> u32 {
    let Some(day) = parse_day(day) else {
        return 0;
    };
    let today = local_day(now_ms, tz);
    u32::try_from((today - day).num_days()).unwrap_or(0)
}

/// One complete match for the daily chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayedMatch {
    pub started_at_ms: i64,
    pub outcome: Outcome,
}

/// One local day of the daily chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayRow {
    /// `YYYY-MM-DD` in the local timezone.
    pub day: String,
    pub wins: u32,
    pub losses: u32,
    /// Anything that is neither a win nor a loss.
    pub draws: u32,
    /// `None` on a day without matches.
    pub win_rate: Option<u32>,
    /// The win rate trend after the day's last match, kept on days without
    /// one. `None` before the first match.
    pub trend: Option<u32>,
}

/// The span of the trend's exponentially weighted mean, in matches: each
/// match back weighs `1 - 2 / (TREND_SPAN + 1)` of the next, so one 50 back
/// counts about a seventh as much as the newest.
pub const TREND_SPAN: u32 = 50;

/// How many matches before a chart's window feed its trend. One this far
/// back weighs under 0.001%, so more would change nothing visible.
pub const TREND_HISTORY: u32 = 300;

/// An exponentially weighted win rate over matches fed oldest first. The
/// match `k` back weighs `(1 - a)^k` with `a = 2 / (TREND_SPAN + 1)`, and
/// the value divides by the weights' sum, so it needs no seed and is exact
/// from the first match.
struct Trend {
    keep: f64,
    weighted_wins: f64,
    weight: f64,
}

impl Trend {
    fn new() -> Trend {
        Trend {
            keep: 1.0 - 2.0 / (f64::from(TREND_SPAN) + 1.0),
            weighted_wins: 0.0,
            weight: 0.0,
        }
    }

    /// A win counts 1, anything else 0, as every win rate here does.
    fn push(&mut self, outcome: Outcome) {
        let won = if outcome == Outcome::Win { 1.0 } else { 0.0 };
        self.weighted_wins = self.weighted_wins * self.keep + won;
        self.weight = self.weight * self.keep + 1.0;
    }

    /// The trend in whole percent; `None` before the first match.
    fn percent(&self) -> Option<u32> {
        if self.weight == 0.0 {
            return None;
        }
        Some(crate::php_round(100.0 * self.weighted_wins / self.weight, 0) as u32)
    }
}

/// What one local day collects while the matches are walked in order.
struct DayCounts {
    wins: u32,
    losses: u32,
    total: u32,
    trend: Option<u32>,
}

/// Matches bucketed by local day in `tz`, every day from the first to the
/// last day with a match (0.x `GetDailyMatchResults` plus the dashboard's
/// dense day range). No matches give no days.
///
/// `earlier` are the matches before `matches`' window: they only feed each
/// day's trend, so a chart starts on a trend of the player's whole form, not
/// on its first day alone. Both lists are put in time order here.
pub fn daily_results(earlier: &[PlayedMatch], matches: &[PlayedMatch], tz: Tz) -> Vec<DayRow> {
    let mut trend = Trend::new();
    let mut before: Vec<&PlayedMatch> = earlier.iter().collect();
    before.sort_by_key(|played| played.started_at_ms);
    for played in before {
        trend.push(played.outcome);
    }

    let mut ordered: Vec<&PlayedMatch> = matches.iter().collect();
    ordered.sort_by_key(|played| played.started_at_ms);
    let mut buckets: BTreeMap<NaiveDate, DayCounts> = BTreeMap::new();
    for played in ordered {
        trend.push(played.outcome);
        let bucket = buckets
            .entry(local_day(played.started_at_ms, tz))
            .or_insert(DayCounts {
                wins: 0,
                losses: 0,
                total: 0,
                trend: None,
            });
        // Anything neither won nor lost is a draw, an unknown outcome too
        // (0.x `GetDailyMatchResults`).
        match played.outcome {
            Outcome::Win => bucket.wins += 1,
            Outcome::Loss => bucket.losses += 1,
            Outcome::Draw | Outcome::Unknown => {}
        }
        bucket.total += 1;
        bucket.trend = trend.percent();
    }

    let (Some(first), Some(last)) = (
        buckets.keys().next().copied(),
        buckets.keys().next_back().copied(),
    ) else {
        return Vec::new();
    };

    let mut days = Vec::new();
    let mut carried = None;
    let mut day = first;
    while day <= last {
        let row = match buckets.get(&day) {
            Some(bucket) => {
                let record = Record::from_total(bucket.wins, bucket.losses, bucket.total);
                carried = bucket.trend;
                DayRow {
                    day: day.format("%Y-%m-%d").to_string(),
                    wins: record.wins,
                    losses: record.losses,
                    draws: record.draws,
                    win_rate: Some(record.win_rate),
                    trend: carried,
                }
            }
            None => DayRow {
                day: day.format("%Y-%m-%d").to_string(),
                wins: 0,
                losses: 0,
                draws: 0,
                win_rate: None,
                trend: carried,
            },
        };
        days.push(row);
        day += Duration::days(1);
    }
    days
}

/// Which of the player's decks a chart groups a match under: the deck's
/// archetype when it has one (versions and sibling decks of one archetype are
/// one group), else the deck itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckLabel {
    /// Names the group: `archetype:<id>` or `deck:<id>`, so an archetype and a
    /// deck with the same number never meet.
    pub key: String,
    pub name: String,
}

impl DeckLabel {
    pub fn archetype(id: i64, name: &str) -> DeckLabel {
        DeckLabel {
            key: format!("archetype:{id}"),
            name: name.to_string(),
        }
    }

    pub fn deck(id: i64, name: &str) -> DeckLabel {
        DeckLabel {
            key: format!("deck:{id}"),
            name: name.to_string(),
        }
    }
}

/// One complete match for the per-deck breakdown of the daily chart. `deck`
/// is `None` when the match has no deck (a gameless import, a draft).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckMatch {
    pub started_at_ms: i64,
    pub outcome: Outcome,
    pub deck: Option<DeckLabel>,
}

/// What the player's decks did on one local day, one entry per group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayDeck {
    pub key: String,
    pub name: String,
    pub wins: u32,
    pub losses: u32,
    /// Anything that is neither a win nor a loss, as in [`DayRow`].
    pub draws: u32,
}

/// The matches of each local day (`YYYY-MM-DD`, as [`DayRow::day`]) by the
/// deck group played, most matches first and then by name. A day without a
/// match that has a deck has no entry. A group takes the name of its latest
/// match, so a renamed deck shows its current name.
pub fn daily_decks(matches: &[DeckMatch], tz: Tz) -> BTreeMap<String, Vec<DayDeck>> {
    let mut ordered: Vec<&DeckMatch> = matches.iter().collect();
    ordered.sort_by_key(|played| played.started_at_ms);

    let mut days: BTreeMap<NaiveDate, BTreeMap<String, DayDeck>> = BTreeMap::new();
    for played in ordered {
        let Some(deck) = &played.deck else {
            continue;
        };
        let counts = days
            .entry(local_day(played.started_at_ms, tz))
            .or_default()
            .entry(deck.key.clone())
            .or_insert_with(|| DayDeck {
                key: deck.key.clone(),
                name: deck.name.clone(),
                wins: 0,
                losses: 0,
                draws: 0,
            });
        counts.name = deck.name.clone();
        match played.outcome {
            Outcome::Win => counts.wins += 1,
            Outcome::Loss => counts.losses += 1,
            Outcome::Draw | Outcome::Unknown => counts.draws += 1,
        }
    }

    let mut result = BTreeMap::new();
    for (day, groups) in days {
        let mut decks: Vec<DayDeck> = groups.into_values().collect();
        decks.sort_by(|a, b| {
            let a_total = a.wins + a.losses + a.draws;
            let b_total = b.wins + b.losses + b.draws;
            b_total.cmp(&a_total).then_with(|| a.name.cmp(&b.name))
        });
        result.insert(day.format("%Y-%m-%d").to_string(), decks);
    }
    result
}

/// Days of the rolling window the sparkline smooths over (0.x
/// `KpiMatchWinRate`).
pub const SPARKLINE_DAYS: usize = 7;

/// The win rate sparkline: for each chart day, the win rate (0 to 100, not
/// rounded) over that day and the 6 before it, only where that window has
/// matches.
pub fn sparkline(days: &[DayRow]) -> Vec<f64> {
    let mut points = Vec::new();
    for index in 0..days.len() {
        let start = (index + 1).saturating_sub(SPARKLINE_DAYS);
        let mut wins = 0u32;
        let mut total = 0u32;
        for day in &days[start..=index] {
            wins += day.wins;
            total += day.wins + day.losses + day.draws;
        }
        if total > 0 {
            points.push(f64::from(wins) / f64::from(total) * 100.0);
        }
    }
    points
}

/// A version's date label (0.x `GetDeckVersionStats`): its local
/// `modified_at` day, then the next version's, or `now` for the latest
/// (`Oct 07 - Oct 09`, `Oct 09 - now`).
pub fn version_date_label(modified_at_ms: i64, next_modified_at_ms: Option<i64>, tz: Tz) -> String {
    let from = local_day(modified_at_ms, tz).format("%b %d");
    match next_modified_at_ms {
        Some(next) => format!("{from} - {}", local_day(next, tz).format("%b %d")),
        None => format!("{from} - now"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTC milliseconds of `YYYY-MM-DDTHH:MM:SS.mmm`.
    fn utc(text: &str) -> i64 {
        let parsed = chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.3f")
            .unwrap_or_else(|error| panic!("{text}: {error}"));
        parsed.and_utc().timestamp_millis()
    }

    fn range(from: &str, to: &str) -> Option<Range> {
        Some(Range {
            from_ms: utc(from),
            to_ms: utc(to),
        })
    }

    fn played(at: &str, outcome: Outcome) -> PlayedMatch {
        PlayedMatch {
            started_at_ms: utc(at),
            outcome,
        }
    }

    fn day(day: &str, wins: u32, losses: u32, draws: u32) -> DayRow {
        day_with_trend(day, wins, losses, draws, None)
    }

    fn day_with_trend(day: &str, wins: u32, losses: u32, draws: u32, trend: Option<u32>) -> DayRow {
        let total = wins + losses + draws;
        DayRow {
            day: day.to_string(),
            wins,
            losses,
            draws,
            win_rate: (total > 0).then(|| winrate_percent(wins, losses, draws)),
            trend,
        }
    }

    /// `count` matches a minute apart from `start`, all with `outcome`.
    fn run_of(start: &str, count: usize, outcome: Outcome) -> Vec<PlayedMatch> {
        (0..count)
            .map(|n| PlayedMatch {
                started_at_ms: utc(start) + n as i64 * 60_000,
                outcome,
            })
            .collect()
    }

    const LONDON: Tz = chrono_tz::Europe::London;
    const LOS_ANGELES: Tz = chrono_tz::America::Los_Angeles;

    // Win rates and records (0.x MatchRecordTest)

    #[test]
    fn a_win_rate_divides_wins_by_every_match_played_draws_included() {
        assert_eq!(winrate_percent(83, 98, 6), 44);
        assert_eq!(winrate_percent(1, 1, 1), 33);
        assert_eq!(winrate_percent(0, 0, 0), 0);
        assert_eq!(winrate_percent(3, 0, 0), 100);
    }

    #[test]
    fn the_two_0x_percentages_round_their_own_float() {
        // 0.x Winrate: 100 * 23 / 40 is exactly 57.5.
        assert_eq!(winrate_percent(23, 17, 0), 58);
        // 0.x SampleRate: 23 / 40 * 100 is 57.49999999999999.
        assert_eq!(sample_rate_percent(23, 40), Some(57));
        assert_eq!(sample_rate_percent(1, 8), Some(13));
        assert_eq!(sample_rate_percent(2, 3), Some(67));
        assert_eq!(sample_rate_percent(5, 0), None);
    }

    #[test]
    fn a_win_rate_rounds_half_away_from_zero_like_php() {
        // 1 / 8 = 12.5%, 5 / 8 = 62.5%.
        assert_eq!(winrate_percent(1, 7, 0), 13);
        assert_eq!(winrate_percent(5, 3, 0), 63);
        // 2 / 3 = 66.67%.
        assert_eq!(winrate_percent(2, 1, 0), 67);
    }

    #[test]
    fn a_record_derives_draws_from_a_total() {
        let record = Record::from_total(1, 1, 3);
        assert_eq!(
            record,
            Record {
                wins: 1,
                losses: 1,
                draws: 1,
                total: 3,
                win_rate: 33
            }
        );
    }

    #[test]
    fn a_record_never_has_negative_draws() {
        let record = Record::from_total(2, 2, 3);
        assert_eq!(record.draws, 0);
        assert_eq!(record.total, 4);
    }

    #[test]
    fn a_record_label_shows_draws_only_when_there_are_any() {
        assert_eq!(record_label(&Record::from_counts(3, 2, 0)), "3 - 2");
        assert_eq!(record_label(&Record::from_counts(3, 2, 1)), "3 - 2 - 1");
        let empty = Record::default();
        assert!(empty.is_empty());
        assert_eq!(empty.win_rate, 0);
        assert_eq!(record_label(&empty), "0 - 0");
    }

    #[test]
    fn records_add_up() {
        let sum = Record::from_counts(6, 4, 2).plus(Record::from_counts(18, 2, 0));
        assert_eq!(
            sum,
            Record {
                wins: 24,
                losses: 6,
                draws: 2,
                total: 32,
                win_rate: 75
            }
        );
    }

    #[test]
    fn game_play_draw_and_boarding_rates() {
        assert_eq!(
            GameRecord::new(1, 1).plus(GameRecord::new(2, 0)),
            GameRecord {
                won: 3,
                lost: 1,
                win_rate: 75
            }
        );
        let play_draw = PlayDraw::new(3, 1, 1, 1, 7);
        assert_eq!(
            play_draw,
            PlayDraw {
                otp_won: 3,
                otp_lost: 1,
                otp_rate: 75,
                otd_won: 1,
                otd_lost: 1,
                otd_rate: 50,
                play_draw_games: 7,
            }
        );
        assert_eq!(
            play_draw.plus(PlayDraw::new(0, 0, 1, 0, 1)),
            PlayDraw::new(3, 1, 2, 1, 8)
        );
        // 0.x GetBoardingSplitTest: lost game one, won both after boarding.
        assert_eq!(
            BoardingSplit::new(0, 1, 2, 0),
            BoardingSplit {
                game_one_won: 0,
                game_one_lost: 1,
                game_one_rate: 0,
                post_board_won: 2,
                post_board_lost: 0,
                post_board_rate: 100,
                delta: 100,
                games: 3,
            }
        );
        assert_eq!(BoardingSplit::new(0, 0, 0, 0), BoardingSplit::default());
    }

    // Timeframes (0.x TimeframeRange)

    #[test]
    fn preset_timeframes_cover_whole_local_days_up_to_the_end_of_today() {
        let now = utc("2026-10-07T12:00:00.000");
        let end = "2026-10-07T23:59:59.999";
        assert_eq!(
            timeframe_range("week", now, chrono_tz::UTC),
            range("2026-09-30T00:00:00.000", end)
        );
        assert_eq!(
            timeframe_range("biweekly", now, chrono_tz::UTC),
            range("2026-09-23T00:00:00.000", end)
        );
        assert_eq!(
            timeframe_range("monthly", now, chrono_tz::UTC),
            range("2026-09-07T00:00:00.000", end)
        );
        assert_eq!(
            timeframe_range("year", now, chrono_tz::UTC),
            range("2026-01-01T00:00:00.000", end)
        );
    }

    #[test]
    fn all_time_unknown_and_malformed_keys_have_no_bounds() {
        let now = utc("2026-10-07T12:00:00.000");
        for key in [
            "alltime",
            "",
            "fortnight",
            "2026-02-31..2026-03-01",
            "2026-1-01..2026-01-02",
            "2026-01-01..2026-01-02..2026-01-03",
            "2026-01-01",
            "2026-01-0a..2026-01-02",
        ] {
            assert_eq!(timeframe_range(key, now, chrono_tz::UTC), None, "{key}");
            assert_eq!(previous_range(key, now, chrono_tz::UTC), None, "{key}");
        }
    }

    #[test]
    fn a_custom_range_includes_both_days_in_either_order() {
        let now = utc("2026-10-07T12:00:00.000");
        let expected = range("2026-10-01T00:00:00.000", "2026-10-03T23:59:59.999");
        assert_eq!(
            timeframe_range("2026-10-01..2026-10-03", now, chrono_tz::UTC),
            expected
        );
        assert_eq!(
            timeframe_range("2026-10-03..2026-10-01", now, chrono_tz::UTC),
            expected
        );
    }

    #[test]
    fn day_boundaries_are_local_midnight() {
        // 03:00 UTC on 10 September is 20:00 PDT on the 9th: today is the 9th.
        let now = utc("2026-09-10T03:00:00.000");
        assert_eq!(
            timeframe_range("week", now, LOS_ANGELES),
            range("2026-09-02T07:00:00.000", "2026-09-10T06:59:59.999")
        );
    }

    #[test]
    fn a_window_across_the_spring_clock_change_keeps_local_midnights() {
        // British Summer Time starts at 01:00 GMT on Sunday 29 March 2026.
        let now = utc("2026-04-02T12:00:00.000");
        let week = timeframe_range("week", now, LONDON);
        // From midnight GMT on 26 March to the last millisecond of 2 April BST.
        assert_eq!(
            week,
            range("2026-03-26T00:00:00.000", "2026-04-02T22:59:59.999")
        );
        assert_eq!(
            previous_range("week", week.expect("week").from_ms, LONDON),
            range("2026-03-18T00:00:00.000", "2026-03-25T23:59:59.999")
        );
        assert_eq!(
            timeframe_range("monthly", now, LONDON),
            range("2026-03-03T00:00:00.000", "2026-04-02T22:59:59.999")
        );
        let custom = timeframe_range("2026-03-28..2026-03-30", now, LONDON);
        assert_eq!(
            custom,
            range("2026-03-28T00:00:00.000", "2026-03-30T22:59:59.999")
        );
        // Two days after the first, so the previous window starts two days
        // before its last day.
        assert_eq!(
            previous_range(
                "2026-03-28..2026-03-30",
                custom.expect("custom").from_ms,
                LONDON
            ),
            range("2026-03-25T00:00:00.000", "2026-03-27T23:59:59.999")
        );
    }

    #[test]
    fn a_window_across_the_autumn_clock_change_keeps_local_midnights() {
        // British Summer Time ends at 01:00 GMT on Sunday 25 October 2026.
        let now = utc("2026-10-27T09:00:00.000");
        assert_eq!(
            timeframe_range("week", now, LONDON),
            range("2026-10-19T23:00:00.000", "2026-10-27T23:59:59.999")
        );
    }

    #[test]
    fn previous_windows_have_the_same_length_and_end_where_the_current_starts() {
        let now = utc("2026-10-07T12:00:00.000");
        let current = |key: &str| {
            timeframe_range(key, now, chrono_tz::UTC)
                .expect("a window")
                .from_ms
        };
        assert_eq!(
            previous_range("week", current("week"), chrono_tz::UTC),
            range("2026-09-22T00:00:00.000", "2026-09-29T23:59:59.999")
        );
        assert_eq!(
            previous_range("biweekly", current("biweekly"), chrono_tz::UTC),
            range("2026-09-08T00:00:00.000", "2026-09-22T23:59:59.999")
        );
        assert_eq!(
            previous_range("monthly", current("monthly"), chrono_tz::UTC),
            range("2026-08-07T00:00:00.000", "2026-09-06T23:59:59.999")
        );
        assert_eq!(
            previous_range("year", current("year"), chrono_tz::UTC),
            range("2025-01-01T00:00:00.000", "2025-12-31T23:59:59.999")
        );
    }

    #[test]
    fn the_default_version_range_is_two_months_in_utc() {
        assert_eq!(
            Some(default_version_range(utc("2026-10-07T12:34:56.000"))),
            range("2026-08-07T00:00:00.000", "2026-10-07T23:59:59.999")
        );
    }

    #[test]
    fn the_default_version_range_overflows_short_months_like_carbon() {
        // Carbon `subMonths(2)`: a day past the end of the target month
        // spills into the next month (checked against Carbon 3.14).
        let start = |now: &str| {
            let from = default_version_range(utc(now)).from_ms;
            DateTime::<Utc>::from_timestamp_millis(from)
                .expect("ms")
                .format("%Y-%m-%d %H:%M")
                .to_string()
        };
        assert_eq!(start("2026-04-30T09:00:00.000"), "2026-03-02 00:00");
        assert_eq!(start("2026-08-31T09:00:00.000"), "2026-07-01 00:00");
        assert_eq!(start("2026-01-31T09:00:00.000"), "2025-12-01 00:00");
        assert_eq!(start("2026-03-31T09:00:00.000"), "2026-01-31 00:00");
    }

    // Daily chart (0.x DashboardChartDataTest)

    #[test]
    fn matches_are_bucketed_by_local_day_and_draws_count_as_played() {
        let days = daily_results(
            &[],
            &[
                // 20:00 PDT on 7 September.
                played("2026-09-08T03:00:00.000", Outcome::Win),
                // 20:30 PDT on the 7th: a draw is a match played.
                played("2026-09-08T03:30:00.000", Outcome::Draw),
                // 10:00 PDT on the 8th.
                played("2026-09-08T17:00:00.000", Outcome::Loss),
            ],
            LOS_ANGELES,
        );
        // Win, draw, loss in order: 100%, then 49%, then 32%.
        assert_eq!(
            days,
            vec![
                day_with_trend("2026-09-07", 1, 0, 1, Some(49)),
                day_with_trend("2026-09-08", 0, 1, 0, Some(32)),
            ]
        );
        assert_eq!(days[0].win_rate, Some(50));
        assert_eq!(days[1].win_rate, Some(0));
    }

    #[test]
    fn chart_days_are_dense_from_the_first_to_the_last_day_with_matches() {
        let days = daily_results(
            &[],
            &[
                played("2026-09-11T10:00:00.000", Outcome::Unknown),
                played("2026-09-08T10:00:00.000", Outcome::Win),
            ],
            chrono_tz::UTC,
        );
        assert_eq!(
            days,
            vec![
                day_with_trend("2026-09-08", 1, 0, 0, Some(100)),
                day_with_trend("2026-09-09", 0, 0, 0, Some(100)),
                day_with_trend("2026-09-10", 0, 0, 0, Some(100)),
                day_with_trend("2026-09-11", 0, 0, 1, Some(49)),
            ]
        );
        assert_eq!(days[1].win_rate, None);
        assert!(daily_results(&[], &[], chrono_tz::UTC).is_empty());
    }

    #[test]
    fn a_midnight_the_clocks_skip_starts_the_day_at_the_end_of_the_gap() {
        // Santiago moves its clocks from 00:00 to 01:00 (-03:00) on
        // 2026-09-06: PHP's startOfDay moves midnight on by the gap.
        let day = NaiveDate::from_ymd_opt(2026, 9, 6).expect("day");
        assert_eq!(
            local_midnight(day, chrono_tz::America::Santiago),
            utc("2026-09-06T04:00:00.000")
        );
        // Lord Howe's half hour gap is at 02:00, so its midnight is ordinary.
        let day = NaiveDate::from_ymd_opt(2026, 10, 4).expect("day");
        assert_eq!(
            local_midnight(day, chrono_tz::Australia::Lord_Howe),
            utc("2026-10-03T13:30:00.000")
        );
    }

    #[test]
    fn chart_days_follow_the_spring_clock_change() {
        let days = daily_results(
            &[],
            &[
                // 00:30 GMT on 29 March, before the clocks go forward.
                played("2026-03-29T00:30:00.000", Outcome::Win),
                // 00:30 BST on 30 March.
                played("2026-03-29T23:30:00.000", Outcome::Loss),
            ],
            LONDON,
        );
        assert_eq!(
            days,
            vec![
                day_with_trend("2026-03-29", 1, 0, 0, Some(100)),
                day_with_trend("2026-03-30", 0, 1, 0, Some(49)),
            ]
        );
    }

    #[test]
    fn the_trend_is_the_weighted_win_rate_of_every_match_so_far() {
        // 20 losses then 20 wins in one day, span 50: r = 49 / 51, the
        // newest win weighs 1 and the oldest loss r^39, so
        // trend = (1 - r^20) / (1 - r^40) = 1 / (1 + r^20) = 0.6900.
        let mut matches = run_of("2026-09-01T10:00:00.000", 20, Outcome::Loss);
        matches.extend(run_of("2026-09-01T12:00:00.000", 20, Outcome::Win));
        let days = daily_results(&[], &matches, chrono_tz::UTC);
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].trend, Some(69));
    }

    #[test]
    fn the_trend_starts_exact_on_the_first_match_and_has_no_value_before_it() {
        assert_eq!(Trend::new().percent(), None);
        let win = daily_results(
            &[],
            &run_of("2026-09-01T10:00:00.000", 1, Outcome::Win),
            chrono_tz::UTC,
        );
        assert_eq!(win[0].trend, Some(100));
        let loss = daily_results(
            &[],
            &run_of("2026-09-01T10:00:00.000", 1, Outcome::Loss),
            chrono_tz::UTC,
        );
        assert_eq!(loss[0].trend, Some(0));
    }

    #[test]
    fn a_day_without_matches_keeps_the_previous_trend() {
        let days = daily_results(
            &[],
            &[
                played("2026-09-01T10:00:00.000", Outcome::Win),
                played("2026-09-01T11:00:00.000", Outcome::Loss),
                played("2026-09-04T10:00:00.000", Outcome::Win),
            ],
            chrono_tz::UTC,
        );
        let trends: Vec<Option<u32>> = days.iter().map(|day| day.trend).collect();
        // Win, loss: 49%. The two empty days keep it; the win lifts it.
        assert_eq!(trends[0], Some(49));
        assert_eq!(trends[1], trends[0]);
        assert_eq!(trends[2], trends[0]);
        assert!(trends[3] > trends[2]);
    }

    #[test]
    fn earlier_matches_feed_the_first_days_trend_without_becoming_days() {
        let window = [played("2026-09-10T10:00:00.000", Outcome::Win)];
        let alone = daily_results(&[], &window, chrono_tz::UTC);
        assert_eq!(alone[0].trend, Some(100));

        let earlier = run_of("2026-08-01T10:00:00.000", 3, Outcome::Loss);
        let with_history = daily_results(&earlier, &window, chrono_tz::UTC);
        assert_eq!(with_history.len(), 1, "earlier matches add no chart day");
        assert_eq!(with_history[0].wins + with_history[0].losses, 1);
        // Three losses then a win: (1) / (1 + r + r^2 + r^3) = 0.2652.
        assert_eq!(with_history[0].trend, Some(27));
    }

    #[test]
    fn matches_are_weighed_in_time_order_whatever_order_they_arrive_in() {
        let in_order = [
            played("2026-09-01T10:00:00.000", Outcome::Loss),
            played("2026-09-01T11:00:00.000", Outcome::Win),
        ];
        let reversed = [in_order[1].clone(), in_order[0].clone()];
        assert_eq!(
            daily_results(&[], &in_order, chrono_tz::UTC),
            daily_results(&[], &reversed, chrono_tz::UTC)
        );
    }

    #[test]
    fn the_trend_keeps_moving_after_thousands_of_matches() {
        // 3000 wins then 100 losses: a cumulative rate would stay near 97%.
        let mut matches = run_of("2026-01-01T00:00:00.000", 3000, Outcome::Win);
        matches.extend(run_of("2026-01-10T00:00:00.000", 100, Outcome::Loss));
        let days = daily_results(&[], &matches, chrono_tz::UTC);
        let last = days.last().and_then(|day| day.trend).expect("a trend");
        assert!(last < 15, "{last}");
    }

    fn deck_match(at: &str, outcome: Outcome, deck: Option<DeckLabel>) -> DeckMatch {
        DeckMatch {
            started_at_ms: utc(at),
            outcome,
            deck,
        }
    }

    fn day_deck(key: &str, name: &str, wins: u32, losses: u32, draws: u32) -> DayDeck {
        DayDeck {
            key: key.to_string(),
            name: name.to_string(),
            wins,
            losses,
            draws,
        }
    }

    #[test]
    fn decks_of_one_archetype_are_one_row_whatever_their_ids() {
        let matches = [
            deck_match(
                "2026-09-01T10:00:00.000",
                Outcome::Win,
                Some(DeckLabel::archetype(7, "Murktide")),
            ),
            deck_match(
                "2026-09-01T11:00:00.000",
                Outcome::Loss,
                Some(DeckLabel::archetype(7, "Murktide")),
            ),
            deck_match(
                "2026-09-01T12:00:00.000",
                Outcome::Win,
                Some(DeckLabel::archetype(7, "Murktide")),
            ),
        ];
        let days = daily_decks(&matches, chrono_tz::UTC);
        assert_eq!(
            days["2026-09-01"],
            vec![day_deck("archetype:7", "Murktide", 2, 1, 0)]
        );
    }

    #[test]
    fn a_deck_without_an_archetype_is_its_own_row_and_never_meets_an_archetype_of_the_same_number()
    {
        let matches = [
            deck_match(
                "2026-09-01T10:00:00.000",
                Outcome::Win,
                Some(DeckLabel::deck(7, "Brew")),
            ),
            deck_match(
                "2026-09-01T11:00:00.000",
                Outcome::Loss,
                Some(DeckLabel::archetype(7, "Murktide")),
            ),
        ];
        let days = daily_decks(&matches, chrono_tz::UTC);
        assert_eq!(
            days["2026-09-01"],
            vec![
                day_deck("deck:7", "Brew", 1, 0, 0),
                day_deck("archetype:7", "Murktide", 0, 1, 0),
            ]
        );
    }

    #[test]
    fn a_draw_or_an_unknown_outcome_is_a_draw_and_a_match_without_a_deck_is_left_out() {
        let label = DeckLabel::archetype(1, "Burn");
        let matches = [
            deck_match(
                "2026-09-01T10:00:00.000",
                Outcome::Draw,
                Some(label.clone()),
            ),
            deck_match("2026-09-01T11:00:00.000", Outcome::Unknown, Some(label)),
            deck_match("2026-09-02T10:00:00.000", Outcome::Win, None),
        ];
        let days = daily_decks(&matches, chrono_tz::UTC);
        assert_eq!(
            days["2026-09-01"],
            vec![day_deck("archetype:1", "Burn", 0, 0, 2)]
        );
        assert!(!days.contains_key("2026-09-02"));
    }

    #[test]
    fn rows_run_by_matches_played_then_by_name() {
        let murktide = DeckLabel::archetype(1, "Murktide");
        let burn = DeckLabel::archetype(2, "Burn");
        let affinity = DeckLabel::archetype(3, "Affinity");
        let matches = [
            deck_match("2026-09-01T09:00:00.000", Outcome::Win, Some(affinity)),
            deck_match(
                "2026-09-01T10:00:00.000",
                Outcome::Win,
                Some(murktide.clone()),
            ),
            deck_match("2026-09-01T11:00:00.000", Outcome::Win, Some(burn.clone())),
            deck_match("2026-09-01T12:00:00.000", Outcome::Win, Some(burn)),
            deck_match("2026-09-01T13:00:00.000", Outcome::Loss, Some(murktide)),
        ];
        let days = daily_decks(&matches, chrono_tz::UTC);
        let names: Vec<&str> = days["2026-09-01"].iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["Burn", "Murktide", "Affinity"]);
    }

    #[test]
    fn days_are_local_days() {
        let label = DeckLabel::archetype(1, "Burn");
        // 06:30 UTC on the 2nd is 23:30 on the 1st in Los Angeles.
        let matches = [
            deck_match(
                "2026-09-02T06:30:00.000",
                Outcome::Loss,
                Some(label.clone()),
            ),
            deck_match("2026-09-02T08:00:00.000", Outcome::Win, Some(label)),
        ];
        let days = daily_decks(&matches, LOS_ANGELES);
        assert_eq!(
            days["2026-09-01"],
            vec![day_deck("archetype:1", "Burn", 0, 1, 0)]
        );
        assert_eq!(
            days["2026-09-02"],
            vec![day_deck("archetype:1", "Burn", 1, 0, 0)]
        );
    }

    #[test]
    fn a_group_shows_the_name_of_its_latest_match() {
        let matches = [
            deck_match(
                "2026-09-01T10:00:00.000",
                Outcome::Win,
                Some(DeckLabel::deck(4, "Old name")),
            ),
            deck_match(
                "2026-09-01T11:00:00.000",
                Outcome::Win,
                Some(DeckLabel::deck(4, "New name")),
            ),
        ];
        let days = daily_decks(&matches, chrono_tz::UTC);
        assert_eq!(
            days["2026-09-01"],
            vec![day_deck("deck:4", "New name", 2, 0, 0)]
        );
    }

    #[test]
    fn no_matches_give_no_days_of_decks() {
        assert!(daily_decks(&[], chrono_tz::UTC).is_empty());
    }

    #[test]
    fn the_sparkline_is_a_seven_day_rolling_win_rate_on_days_with_matches() {
        let mut days = vec![day("2026-09-01", 1, 0, 0)];
        for n in 2..=7 {
            days.push(day(&format!("2026-09-0{n}"), 0, 0, 0));
        }
        days.push(day("2026-09-08", 0, 1, 0));
        // Days 1 to 7 still see the win on day 1; day 8's window (days 2 to
        // 8) only has the loss.
        assert_eq!(
            sparkline(&days),
            vec![100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 0.0]
        );

        let points = sparkline(&[day("2026-09-01", 1, 0, 1), day("2026-09-02", 0, 1, 0)]);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0], 50.0);
        assert!((points[1] - 100.0 / 3.0).abs() < 1e-9, "{points:?}");
        assert!(sparkline(&[]).is_empty());
        assert!(sparkline(&[day("2026-09-01", 0, 0, 0)]).is_empty());
    }

    #[test]
    fn days_ago_counts_local_days_to_today() {
        let now = utc("2026-09-10T12:00:00.000");
        assert_eq!(days_ago("2026-09-10", now, chrono_tz::UTC), 0);
        assert_eq!(days_ago("2026-09-08", now, chrono_tz::UTC), 2);
        assert_eq!(
            days_ago("2026-09-11", now, chrono_tz::UTC),
            0,
            "a day ahead"
        );
        assert_eq!(days_ago("not a day", now, chrono_tz::UTC), 0);
        // 03:00 UTC on the 10th is still the 9th in Los Angeles.
        let early = utc("2026-09-10T03:00:00.000");
        assert_eq!(days_ago("2026-09-08", early, LOS_ANGELES), 1);
    }

    // Version labels (0.x GetDeckVersionStats)

    #[test]
    fn version_labels_run_to_the_next_version_or_now_in_local_days() {
        assert_eq!(
            version_date_label(
                utc("2026-10-07T10:00:00.000"),
                Some(utc("2026-10-09T10:00:00.000")),
                chrono_tz::UTC
            ),
            "Oct 07 - Oct 09"
        );
        // 03:00 UTC on 7 October is 20:00 PDT on the 6th.
        assert_eq!(
            version_date_label(utc("2026-10-07T03:00:00.000"), None, LOS_ANGELES),
            "Oct 06 - now"
        );
    }
}
