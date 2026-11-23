//! Arena rank views (Arena spec 15): a ladder's rank line for the dashboard,
//! and an Arena deck's share of the constructed climb (`deck_rank`).
//!
//! The line is built on read from the stored rank readings and the account's
//! ranked matches; the rules (gaps, attribution, the deck climb) are
//! `arena_match::rank_line`'s.

use std::collections::{HashMap, HashSet};

use arena_match::rank_band::{BandMatch, BandReading, RankBand, match_bands};
use arena_match::rank_line::{
    DeckClimb, GapKind, Observation, RankLine, claimable, climb_text, deck_climb, rank_label,
    rank_line, rank_steps,
};
use arena_protocol::ArenaEvent;
use arena_protocol::clock::season_time_ms;
use chrono::DateTime;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use store::arena_rank::{RankedMatchRow, observation, placed};
use store::{Connection, StoreError};
use tauri::State;

use crate::bridge::lock;
use crate::commands::{CommandError, Startup, ready, storage};
use crate::decks::deck_best_of;
use crate::logging::Timer;
use crate::platform::BestOfView;

/// The two Arena ladders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LadderView {
    Constructed,
    Limited,
}

impl LadderView {
    /// The `arena_rank_observations.ladder` value.
    fn key(self) -> &'static str {
        match self {
            LadderView::Constructed => "constructed",
            LadderView::Limited => "limited",
        }
    }
}

/// What happened between two readings (spec 15's gap rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum GapKindView {
    Attributed,
    PlayedElsewhere,
    Shared,
    Quiet,
}

impl From<&GapKind> for GapKindView {
    fn from(kind: &GapKind) -> GapKindView {
        match kind {
            GapKind::Attributed => GapKindView::Attributed,
            GapKind::PlayedElsewhere => GapKindView::PlayedElsewhere,
            GapKind::Shared => GapKindView::Shared,
            GapKind::Quiet => GapKindView::Quiet,
        }
    }
}

/// One rank reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RankPointView {
    #[specta(type = specta_typescript::Number)]
    pub at_ms: i64,
    /// Steps from Bronze 4 step 0, negative in Spark
    /// (`arena_match::rank_line::rank_steps`).
    pub steps: i32,
    pub label: String,
    pub mythic: bool,
}

/// The stretch between points `from` and `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RankGapView {
    pub from: u32,
    pub to: u32,
    pub kind: GapKindView,
    pub change_steps: i32,
    /// The kind every match in the gap is; `None` for a played-elsewhere
    /// gap, a gap mixing kinds, or a match of no known kind.
    pub best_of: Option<BestOfView>,
    /// An attributed gap whose match is the deck's (always false without a
    /// deck).
    pub of_deck: bool,
}

/// A ranked match on the line (in a gap, or after the last reading).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RankMatchView {
    pub token: String,
    /// The match's end, else its start.
    #[specta(type = specta_typescript::Number)]
    pub at_ms: i64,
    pub outcome: String,
    pub of_deck: bool,
    /// Bo1 or Bo3; `None` when the match has no known kind.
    pub best_of: Option<BestOfView>,
    /// Not placed by the log (a manual match placed by its claim).
    pub manual: bool,
    /// The index into `RankLineView.gaps` the match was counted in; `None`
    /// when it came after the last reading (pending).
    pub gap: Option<u32>,
}

/// One ladder's line in the newest season the account has readings of.
/// `matches` is in log order: by gap, then the pending ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RankLineView {
    pub ladder: LadderView,
    pub season: Option<u32>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub season_start_ms: Option<i64>,
    #[specta(type = Option<specta_typescript::Number>)]
    pub season_end_ms: Option<i64>,
    /// The last reading's label.
    pub current: Option<String>,
    pub points: Vec<RankPointView>,
    pub gaps: Vec<RankGapView>,
    pub matches: Vec<RankMatchView>,
}

/// A deck's share of the constructed climb this season.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckClimbView {
    /// `climbed`, `dropped` or `none`.
    pub direction: String,
    /// `1 tier, 3 steps`, `2 tiers`, `1 step` or `no change`.
    pub text: String,
    /// `and 5.0% in Mythic` when the deck moved a Mythic percentile.
    pub mythic_text: Option<String>,
    /// How many gaps were attributed to the deck.
    pub attributed: u32,
}

/// An Arena deck's rank (`deck_rank`): the account's constructed line with
/// the deck's gaps and matches marked, and the deck's climb this season.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct DeckRankView {
    pub line: RankLineView,
    pub climb: DeckClimbView,
}

/// A constructed gap named by its end: the reading that closes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RankGapKey {
    #[specta(type = specta_typescript::Number)]
    pub file_id: i64,
    #[specta(type = specta_typescript::Number)]
    pub byte_offset: u64,
}

/// A gap a manual Arena match can be claimed into ("Rank gap" select).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RankGapOption {
    pub key: RankGapKey,
    /// `Gold 3 to Gold 2, 3 Nov 14:05 to 15:30` in the OS timezone.
    pub label: String,
    pub change_steps: i32,
    /// The manual matches already claimed into it.
    pub claims: u32,
}

/// A built line and what the views need besides it.
struct Built {
    ladder: LadderView,
    season: u32,
    season_start_ms: Option<i64>,
    season_end_ms: Option<i64>,
    line: RankLine,
    /// The account's ranked matches (empty for the limited ladder).
    rows: Vec<RankedMatchRow>,
}

/// `year * 100 + month` of `ms` in `tz` (`arena_match::rank_band`'s month).
fn band_month(ms: i64, tz: Tz) -> u32 {
    let (year, month) = store::metagame::local_month(ms, tz);
    u32::try_from(year).unwrap_or(0) * 100 + month
}

/// The rank band of every ranked match of the Arena account (every Arena
/// account when `None`), by token: the constructed reading of the same
/// month at its place in the log, months in `tz`. A match not in the map is
/// Unranked to callers.
pub(crate) fn match_bands_of(
    conn: &Connection,
    account: Option<i64>,
    tz: Tz,
) -> Result<HashMap<String, RankBand>, StoreError> {
    let accounts = match account {
        Some(id) => vec![id],
        None => store::accounts::accounts(conn, store::Platform::Arena)?
            .into_iter()
            .map(|row| row.id)
            .collect(),
    };
    let mut out = HashMap::new();
    // One account's readings say nothing about another's matches.
    for account_id in accounts {
        let mut readings = Vec::new();
        for stored in store::arena_rank::all_observations(conn, account_id, "constructed")? {
            readings.push(BandReading {
                file_id: stored.file_id,
                offset: stored.byte_offset,
                month: band_month(stored.observed_at_ms, tz),
                class: stored.class,
            });
        }
        let mut matches = Vec::new();
        for row in store::arena_rank::ranked_matches(conn, account_id)? {
            matches.push(BandMatch {
                place: placed(&row).map(|found| found.place),
                month: band_month(row.started_at_ms, tz),
                token: row.token,
            });
        }
        for (token, band) in match_bands(&readings, &matches) {
            out.insert(token, band);
        }
    }
    Ok(out)
}

/// The season's start and end, from the newest season event when it is of
/// `season` (zoneless text read as UTC).
fn season_dates(conn: &Connection, season: u32) -> Result<(Option<i64>, Option<i64>), StoreError> {
    // Arena seasons are global, so the newest season event of any account is used.
    let Some(raw) = store::arena_rank::latest_arena_event(conn, "season")? else {
        return Ok((None, None));
    };
    match serde_json::from_str::<ArenaEvent>(&raw) {
        Ok(ArenaEvent::Season {
            ordinal,
            start_text,
            end_text,
        }) if ordinal == season => Ok((season_time_ms(&start_text), season_time_ms(&end_text))),
        _ => Ok((None, None)),
    }
}

/// The ladder's line in the account's newest season, or `None` without a
/// reading. Limited matches are not tracked, so the limited line has no
/// matches and every gap is quiet: it joins the readings and says nothing
/// about matches.
fn build(
    conn: &Connection,
    account_id: i64,
    ladder: LadderView,
) -> Result<Option<Built>, StoreError> {
    let Some(season) = store::arena_rank::newest_season(conn, account_id, ladder.key())? else {
        return Ok(None);
    };
    let stored = store::arena_rank::observations(conn, account_id, ladder.key(), season)?;
    let observations: Vec<Observation> = stored.into_iter().map(observation).collect();
    let rows = match ladder {
        LadderView::Constructed => store::arena_rank::ranked_matches(conn, account_id)?,
        LadderView::Limited => Vec::new(),
    };
    let mut ranked = Vec::new();
    for row in &rows {
        if let Some(found) = placed(row) {
            ranked.push(found);
        }
    }
    let mut line = rank_line(observations, &ranked);
    if line.points.is_empty() {
        return Ok(None);
    }
    if ladder == LadderView::Limited {
        for gap in &mut line.gaps {
            gap.kind = GapKind::Quiet;
            gap.tokens.clear();
        }
        line.pending.clear();
    }
    let (season_start_ms, season_end_ms) = season_dates(conn, season)?;
    Ok(Some(Built {
        ladder,
        season,
        season_start_ms,
        season_end_ms,
        line,
        rows,
    }))
}

/// The tokens of the deck's ranked matches, only those of `best_of` when
/// given (none without a deck).
fn deck_tokens(built: &Built, deck_id: Option<i64>, best_of: Option<store::BestOf>) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Some(deck_id) = deck_id {
        for row in &built.rows {
            if row.deck_id == Some(deck_id) && (best_of.is_none() || row.best_of == best_of) {
                tokens.push(row.token.clone());
            }
        }
    }
    tokens
}

/// The kind every one of `tokens` is, `None` for no tokens, mixed kinds, or
/// a match of no known kind.
fn gap_best_of(built: &Built, tokens: &[String]) -> Option<BestOfView> {
    let mut kind: Option<store::BestOf> = None;
    for token in tokens {
        let row = built.rows.iter().find(|row| &row.token == token)?;
        let found = row.best_of?;
        if kind.is_some_and(|seen| seen != found) {
            return None;
        }
        kind = Some(found);
    }
    kind.map(BestOfView::from)
}

fn line_view(built: &Built, deck_id: Option<i64>) -> RankLineView {
    let deck: HashSet<String> = deck_tokens(built, deck_id, None).into_iter().collect();
    let mut points = Vec::new();
    for point in &built.line.points {
        points.push(RankPointView {
            at_ms: point.at_ms,
            steps: rank_steps(&point.class, point.level, point.step).unwrap_or(0),
            label: rank_label(point),
            mythic: point.class == "Mythic",
        });
    }
    let mut gaps = Vec::new();
    for gap in &built.line.gaps {
        gaps.push(RankGapView {
            from: gap.from as u32,
            to: gap.to as u32,
            kind: GapKindView::from(&gap.kind),
            change_steps: gap.change_steps,
            best_of: gap_best_of(built, &gap.tokens),
            of_deck: gap.kind == GapKind::Attributed
                && gap.tokens.iter().any(|token| deck.contains(token)),
        });
    }
    // In log order: each gap's matches, then the pending ones (never by time).
    let mut placed: Vec<(&String, Option<u32>)> = Vec::new();
    for (index, gap) in built.line.gaps.iter().enumerate() {
        for token in &gap.tokens {
            placed.push((token, Some(index as u32)));
        }
    }
    for token in &built.line.pending {
        placed.push((token, None));
    }
    let mut matches = Vec::new();
    for (token, gap) in placed {
        let Some(row) = built.rows.iter().find(|row| &row.token == token) else {
            continue;
        };
        matches.push(RankMatchView {
            token: row.token.clone(),
            at_ms: row.ended_at_ms.unwrap_or(row.started_at_ms),
            outcome: row.outcome.clone(),
            of_deck: deck.contains(&row.token),
            best_of: row.best_of.map(BestOfView::from),
            manual: row.log_place.is_none(),
            gap,
        });
    }
    RankLineView {
        ladder: built.ladder,
        season: Some(built.season),
        season_start_ms: built.season_start_ms,
        season_end_ms: built.season_end_ms,
        current: built.line.points.last().map(rank_label),
        points,
        gaps,
        matches,
    }
}

/// One ladder's line for the account, its gaps and matches marked for
/// `deck_id` when given. `None` with no reading this season.
pub(crate) fn rank_line_view(
    conn: &Connection,
    account_id: i64,
    ladder: LadderView,
    deck_id: Option<i64>,
) -> Result<Option<RankLineView>, StoreError> {
    Ok(build(conn, account_id, ladder)?.map(|built| line_view(&built, deck_id)))
}

/// The deck's Arena account, or `None` for an MTGO deck (or no deck).
pub(crate) fn arena_account_of_deck(
    conn: &Connection,
    deck_id: i64,
) -> Result<Option<i64>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT a.id FROM decks d JOIN accounts a ON a.id = d.account_id
         WHERE d.id = ?1 AND a.platform = 'arena'",
    )?;
    let mut rows = stmt.query_map([deck_id], |row| row.get::<_, i64>(0))?;
    Ok(rows.next().transpose()?)
}

/// `and 5.0% in Mythic`: one decimal, a minus when it fell.
fn mythic_text(change: f64) -> String {
    // Keep a tiny fall from reading as "-0.0".
    let change = if change.abs() < 0.05 { 0.0 } else { change };
    format!("and {change:.1}% in Mythic")
}

/// The direction and the text both come from the climb in tiers and steps
/// (`DeckClimb::tiers`, `tier_steps`), never the raw step sum: with Spark's
/// five-step tiers a Spark tier up and a Bronze tier down is -1 step but no
/// change. The two share a sign, so the tiers' sign decides when there are
/// any.
fn climb_view(climb: &DeckClimb) -> DeckClimbView {
    let sign = if climb.tiers != 0 {
        climb.tiers.signum()
    } else {
        climb.tier_steps.signum()
    };
    let direction = match sign {
        1 => "climbed",
        -1 => "dropped",
        _ => "none",
    };
    DeckClimbView {
        direction: direction.to_string(),
        text: climb_text(climb.tiers, climb.tier_steps),
        mythic_text: climb.mythic_change.map(mythic_text),
        attributed: climb.attributed,
    }
}

/// An Arena deck's constructed line with its matches marked, and its climb.
/// `None` for an MTGO deck or an account with no constructed reading. The
/// climb counts only the deck's matches of `best_of` when given.
pub(crate) fn deck_rank_view(
    conn: &Connection,
    deck_id: i64,
    best_of: Option<store::BestOf>,
) -> Result<Option<DeckRankView>, StoreError> {
    let Some(account_id) = arena_account_of_deck(conn, deck_id)? else {
        return Ok(None);
    };
    let Some(built) = build(conn, account_id, LadderView::Constructed)? else {
        return Ok(None);
    };
    let climb = deck_climb(&built.line, &deck_tokens(&built, Some(deck_id), best_of));
    Ok(Some(DeckRankView {
        line: line_view(&built, Some(deck_id)),
        climb: climb_view(&climb),
    }))
}

/// The constructed gaps of the account's newest season a manual match can
/// be claimed into, newest first: those played elsewhere (no match, the
/// rank moved) and those that hold only claimed manual matches. Times in
/// `tz`, the OS timezone.
pub(crate) fn played_elsewhere_gaps(
    conn: &Connection,
    account_id: i64,
    tz: Tz,
) -> Result<Vec<RankGapOption>, StoreError> {
    let Some(built) = build(conn, account_id, LadderView::Constructed)? else {
        return Ok(Vec::new());
    };
    let mut ranked = Vec::new();
    for row in &built.rows {
        if let Some(found) = placed(row) {
            ranked.push(found);
        }
    }
    let mut options = Vec::new();
    for claimable_gap in claimable(&built.line, &ranked).into_iter().rev() {
        let gap = &built.line.gaps[claimable_gap.gap];
        let claims = claimable_gap.claims;
        let from = &built.line.points[gap.from];
        let to = &built.line.points[gap.to];
        options.push(RankGapOption {
            key: RankGapKey {
                file_id: to.file_id,
                byte_offset: to.offset,
            },
            label: format!(
                "{} to {}, {}",
                rank_label(from),
                rank_label(to),
                span_text(from.at_ms, to.at_ms, tz)
            ),
            change_steps: gap.change_steps,
            claims,
        });
    }
    Ok(options)
}

/// `3 Nov 14:05 to 15:30` in `tz`; the day again when the end is on
/// another day (`3 Nov 23:50 to 4 Nov 00:20`).
fn span_text(from_ms: i64, to_ms: i64, tz: Tz) -> String {
    let local = |ms: i64| DateTime::from_timestamp_millis(ms).map(|at| at.with_timezone(&tz));
    let (Some(from), Some(to)) = (local(from_ms), local(to_ms)) else {
        return String::new();
    };
    let end_pattern = if from.date_naive() == to.date_naive() {
        "%H:%M"
    } else {
        "%-d %b %H:%M"
    };
    format!(
        "{} to {}",
        from.format("%-d %b %H:%M"),
        to.format(end_pattern)
    )
}

#[tauri::command]
#[specta::specta]
pub fn deck_rank(
    deck_id: u32,
    best_of: Option<BestOfView>,
    startup: State<'_, Startup>,
) -> Result<Option<DeckRankView>, CommandError> {
    let _timer = Timer::start("deck_rank");
    let conn = lock(&ready(&startup)?.reader);
    let deck_id = i64::from(deck_id);
    let kind = deck_best_of(&conn, deck_id, best_of)?;
    deck_rank_view(&conn, deck_id, kind).map_err(storage)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use fixture_tools::TestDir;

    fn reading(offset: u64, class: &str, level: u32, won: u32) -> Observation {
        Observation {
            file_id: 1,
            offset,
            at_ms: 0,
            class: class.to_string(),
            level,
            step: 0,
            won,
            lost: 0,
            mythic_percentile: None,
            mythic_placement: None,
        }
    }

    fn deck_match(token: &str, offset: u64) -> arena_match::rank_line::RankedMatch {
        arena_match::rank_line::RankedMatch {
            token: token.to_string(),
            place: arena_match::rank_line::MatchPlace::Log { file_id: 1, offset },
        }
    }

    /// The deck climbs a Spark tier (5 steps) and drops a Bronze tier (6
    /// steps): the view says what the net -1 step is, a drop of 1 step.
    #[test]
    fn a_spark_tier_up_and_a_bronze_tier_down_drops_a_step() {
        let line = rank_line(
            vec![
                reading(10, "Spark", 4, 0),
                reading(30, "Spark", 3, 1),
                reading(50, "Bronze", 3, 1),
                reading(70, "Bronze", 4, 2),
            ],
            &[deck_match("a", 20), deck_match("b", 60)],
        );
        let climb = deck_climb(&line, &["a".to_string(), "b".to_string()]);
        assert_eq!(climb.steps, -1);
        let view = climb_view(&climb);
        assert_eq!(
            (view.direction.as_str(), view.text.as_str()),
            ("dropped", "1 step")
        );

        // The mirror: drop a Spark tier, climb a Bronze tier, +1 step.
        let line = rank_line(
            vec![
                reading(10, "Spark", 3, 0),
                reading(30, "Spark", 4, 1),
                reading(50, "Bronze", 4, 1),
                reading(70, "Bronze", 3, 2),
            ],
            &[deck_match("a", 20), deck_match("b", 60)],
        );
        let climb = deck_climb(&line, &["a".to_string(), "b".to_string()]);
        assert_eq!(climb.steps, 1);
        let view = climb_view(&climb);
        assert_eq!(
            (view.direction.as_str(), view.text.as_str()),
            ("climbed", "1 step")
        );
    }

    #[test]
    fn the_direction_follows_the_climb_text() {
        let climb = |steps, tiers, tier_steps| DeckClimb {
            steps,
            tiers,
            tier_steps,
            mythic_change: None,
            attributed: 1,
        };
        let view = climb_view(&climb(-1, 0, -1));
        assert_eq!(
            (view.direction.as_str(), view.text.as_str()),
            ("dropped", "1 step")
        );
        let view = climb_view(&climb(7, 1, 2));
        assert_eq!(
            (view.direction.as_str(), view.text.as_str()),
            ("climbed", "1 tier, 2 steps")
        );
    }

    pub(crate) fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        (dir, conn)
    }

    pub(crate) fn arena_account(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id')",
            [],
        )
        .expect("arena account");
        conn.last_insert_rowid()
    }

    fn deck(conn: &Connection, account: i64, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES (?1, ?2, ?1, 'CPIONEER', 0, 0, 'file')",
            store::params![name, account],
        )
        .expect("deck");
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, 'Q', 0)",
            [id],
        )
        .expect("version");
        id
    }

    fn log_file(conn: &Connection) {
        conn.execute(
            "INSERT OR IGNORE INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4)",
            [],
        )
        .expect("file");
    }

    fn event(conn: &Connection, offset: i64, kind: &str, token: Option<&str>, raw: &str) {
        log_file(conn);
        conn.execute(
            "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind, match_token,
                 logged_at, raw, compressed) VALUES (1, ?1, 0, ?2, ?3, 1, ?4, 0)",
            store::params![offset, kind, token, raw.as_bytes()],
        )
        .expect("event");
    }

    #[allow(clippy::too_many_arguments)]
    fn observe(
        conn: &Connection,
        account: i64,
        ladder: &str,
        offset: i64,
        class: &str,
        level: u32,
        step: u32,
        won: u32,
        lost: u32,
    ) {
        log_file(conn);
        conn.execute(
            "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at, class,
                 level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
             VALUES (?1, ?2, 95, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
            store::params![
                account,
                ladder,
                offset * 1000,
                class,
                level,
                step,
                won,
                lost,
                offset
            ],
        )
        .expect("observation");
    }

    /// A ranked match of `account`, on `deck_id`'s version when given, placed
    /// in the log at `offset` when given.
    fn ranked(
        conn: &Connection,
        account: i64,
        token: &str,
        deck_id: Option<i64>,
        offset: Option<i64>,
        outcome: &str,
        ended_at: i64,
    ) {
        conn.execute(
            "INSERT INTO matches (token, account_id, source, format, match_type, state, outcome,
                 started_at, ended_at, last_activity_at, revision, derived_version,
                 deck_version_id)
             VALUES (?1, ?2, 'log', 'CPIONEER', 'ranked', 'complete', ?3, ?4 - 600000, ?4, ?4,
                 1, 1, (SELECT id FROM deck_versions WHERE deck_id = ?5))",
            store::params![token, account, outcome, ended_at, deck_id],
        )
        .expect("match");
        if let Some(offset) = offset {
            event(conn, offset, "game_info", Some(token), "{}");
        }
    }

    /// The seeded Arena account: deck `Qdeck` won `q-a` (alone in the first
    /// gap, +4 steps) and `q-b` (alone in the third gap, +3 steps); the
    /// second gap moved with no match (played elsewhere); `q-c` (another
    /// deck) came after the last reading; `q-m` is a manual match with no
    /// claim. Limited has two readings. Returns (account, deck).
    pub(crate) fn seed(conn: &Connection) -> (i64, i64) {
        let account = arena_account(conn);
        let own = deck(conn, account, "Qdeck");
        let other = deck(conn, account, "Qother");
        let season = ArenaEvent::Season {
            ordinal: 95,
            start_text: "2026-10-31T19:05:00".to_string(),
            end_text: "2026-11-30T20:00:00".to_string(),
        };
        let raw = serde_json::to_string(&season).expect("season json");
        event(conn, 50, "season", None, &raw);
        observe(conn, account, "constructed", 100, "Gold", 2, 0, 0, 0);
        ranked(conn, account, "q-a", Some(own), Some(200), "win", 2_000_000);
        observe(conn, account, "constructed", 300, "Gold", 2, 4, 1, 0);
        observe(conn, account, "constructed", 500, "Gold", 1, 3, 2, 0);
        ranked(conn, account, "q-b", Some(own), Some(600), "win", 6_000_000);
        observe(conn, account, "constructed", 700, "Platinum", 4, 0, 3, 0);
        ranked(
            conn,
            account,
            "q-c",
            Some(other),
            Some(800),
            "loss",
            8_000_000,
        );
        ranked(conn, account, "q-m", Some(own), None, "win", 9_000_000);
        observe(conn, account, "limited", 150, "Silver", 3, 1, 1, 0);
        observe(conn, account, "limited", 350, "Silver", 2, 0, 2, 0);
        (account, own)
    }

    fn labels(line: &RankLineView) -> Vec<String> {
        line.points.iter().map(|p| p.label.clone()).collect()
    }

    fn kinds(line: &RankLineView) -> Vec<GapKindView> {
        line.gaps.iter().map(|g| g.kind).collect()
    }

    #[test]
    fn the_constructed_line_has_points_gaps_and_matches() {
        let (_dir, conn) = db("rank-constructed");
        let (account, _) = seed(&conn);
        let line = rank_line_view(&conn, account, LadderView::Constructed, None)
            .expect("read")
            .expect("a line");
        assert_eq!(line.ladder, LadderView::Constructed);
        assert_eq!(line.season, Some(95));
        assert_eq!(line.season_start_ms, Some(1_793_473_500_000));
        assert_eq!(line.season_end_ms, Some(1_796_068_800_000));
        assert_eq!(
            labels(&line),
            vec!["Gold 2", "Gold 2", "Gold 1", "Platinum 4"]
        );
        assert_eq!(line.current.as_deref(), Some("Platinum 4"));
        let steps: Vec<i32> = line.points.iter().map(|p| p.steps).collect();
        assert_eq!(steps, vec![60, 64, 69, 72]);
        assert_eq!(line.points[0].at_ms, 100_000);
        assert!(line.points.iter().all(|p| !p.mythic));
        assert_eq!(
            kinds(&line),
            vec![
                GapKindView::Attributed,
                GapKindView::PlayedElsewhere,
                GapKindView::Attributed
            ]
        );
        let changes: Vec<i32> = line.gaps.iter().map(|g| g.change_steps).collect();
        assert_eq!(changes, vec![4, 5, 3]);
        assert_eq!((line.gaps[2].from, line.gaps[2].to), (2, 3));
        assert!(line.gaps.iter().all(|g| !g.of_deck), "no deck asked for");
        let tokens: Vec<&str> = line.matches.iter().map(|m| m.token.as_str()).collect();
        assert_eq!(
            tokens,
            vec!["q-a", "q-b", "q-c"],
            "the unclaimed manual is out"
        );
        let gaps: Vec<Option<u32>> = line.matches.iter().map(|m| m.gap).collect();
        assert_eq!(gaps, vec![Some(0), Some(2), None]);
        assert_eq!(line.matches[0].at_ms, 2_000_000);
        assert_eq!(line.matches[2].outcome, "loss");
        assert!(line.matches.iter().all(|m| !m.of_deck && !m.manual));
    }

    #[test]
    fn deck_rank_marks_the_decks_matches_and_climbs() {
        let (_dir, conn) = db("rank-deck");
        let (_, own) = seed(&conn);
        let rank = deck_rank_view(&conn, own, None)
            .expect("read")
            .expect("a rank");
        assert_eq!(rank.climb.direction, "climbed");
        assert_eq!(rank.climb.text, "1 tier, 1 step");
        assert_eq!(rank.climb.mythic_text, None);
        assert_eq!(rank.climb.attributed, 2);
        let of_deck: Vec<bool> = rank.line.gaps.iter().map(|g| g.of_deck).collect();
        assert_eq!(of_deck, vec![true, false, true]);
        let matches: Vec<(&str, bool)> = rank
            .line
            .matches
            .iter()
            .map(|m| (m.token.as_str(), m.of_deck))
            .collect();
        assert_eq!(matches, vec![("q-a", true), ("q-b", true), ("q-c", false)]);
    }

    #[test]
    fn a_claimed_manual_match_is_listed_as_manual() {
        let (_dir, conn) = db("rank-claim");
        let (account, own) = seed(&conn);
        conn.execute(
            "INSERT INTO arena_rank_gap_claims (match_token, arena_log_file_id, byte_offset,
                 claimed_at) VALUES ('q-m', 1, 500, 0)",
            [],
        )
        .expect("claim");
        let line = rank_line_view(&conn, account, LadderView::Constructed, Some(own))
            .expect("read")
            .expect("a line");
        assert_eq!(line.gaps[1].kind, GapKindView::Attributed);
        assert!(line.gaps[1].of_deck);
        let manual = line
            .matches
            .iter()
            .find(|m| m.token == "q-m")
            .expect("listed");
        assert!(manual.manual && manual.of_deck);
    }

    /// A match whose times are out of step with the log keeps its log place:
    /// matches list by gap, then pending, never by time.
    #[test]
    fn matches_follow_log_order_not_time() {
        let (_dir, conn) = db("rank-log-order");
        let (account, own) = seed(&conn);
        // q-b (third gap) reads as the oldest, q-c (pending) before q-a.
        conn.execute(
            "UPDATE matches SET started_at = 10, ended_at = 20 WHERE token = 'q-b'",
            [],
        )
        .expect("q-b time");
        conn.execute(
            "UPDATE matches SET started_at = 30, ended_at = 40 WHERE token = 'q-c'",
            [],
        )
        .expect("q-c time");
        let line = rank_line_view(&conn, account, LadderView::Constructed, Some(own))
            .expect("read")
            .expect("a line");
        let placed: Vec<(&str, Option<u32>)> = line
            .matches
            .iter()
            .map(|m| (m.token.as_str(), m.gap))
            .collect();
        assert_eq!(
            placed,
            vec![("q-a", Some(0)), ("q-b", Some(2)), ("q-c", None)]
        );
        assert_eq!(line.matches[1].at_ms, 20);
    }

    #[test]
    fn the_limited_line_is_quiet_between_readings() {
        let (_dir, conn) = db("rank-limited");
        let (account, own) = seed(&conn);
        let line = rank_line_view(&conn, account, LadderView::Limited, Some(own))
            .expect("read")
            .expect("a line");
        assert_eq!(line.ladder, LadderView::Limited);
        assert_eq!(labels(&line), vec!["Silver 3", "Silver 2"]);
        assert_eq!(line.current.as_deref(), Some("Silver 2"));
        // Limited matches are not tracked: one gap per pair of readings, so
        // the chart draws the line, but none says anything about matches.
        assert_eq!(line.gaps.len(), line.points.len() - 1);
        assert_eq!((line.gaps[0].from, line.gaps[0].to), (0, 1));
        assert_eq!(kinds(&line), vec![GapKindView::Quiet]);
        assert!(line.gaps.iter().all(|g| !g.of_deck));
        assert!(line.matches.is_empty());
        assert_eq!(line.season, Some(95));
    }

    #[test]
    fn mtgo_decks_have_no_rank() {
        let (_dir, conn) = db("rank-mtgo");
        seed(&conn);
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)",
            [],
        )
        .expect("mtgo account");
        let mtgo = conn.last_insert_rowid();
        let deck_id = deck(&conn, mtgo, "Qmtgo");
        assert_eq!(deck_rank_view(&conn, deck_id, None).expect("read"), None);
    }

    #[test]
    fn an_arena_deck_with_no_observations_has_no_line() {
        let (_dir, conn) = db("rank-empty");
        let account = arena_account(&conn);
        let deck_id = deck(&conn, account, "Qdeck");
        assert_eq!(deck_rank_view(&conn, deck_id, None).expect("read"), None);
        assert_eq!(
            rank_line_view(&conn, account, LadderView::Constructed, None).expect("read"),
            None
        );
    }

    #[test]
    fn a_season_event_of_another_season_gives_no_dates() {
        let (_dir, conn) = db("rank-old-season");
        let (account, _) = seed(&conn);
        let season = ArenaEvent::Season {
            ordinal: 96,
            start_text: "2026-11-30T20:00:00".to_string(),
            end_text: "2026-12-31T20:00:00".to_string(),
        };
        let raw = serde_json::to_string(&season).expect("season json");
        event(&conn, 900, "season", None, &raw);
        let line = rank_line_view(&conn, account, LadderView::Constructed, None)
            .expect("read")
            .expect("a line");
        assert_eq!(line.season, Some(95));
        assert_eq!((line.season_start_ms, line.season_end_ms), (None, None));
    }

    #[test]
    fn a_span_names_the_day_again_only_across_midnight() {
        // 2026-11-03 14:05 and 15:30 UTC, then 2026-11-04 00:20 UTC.
        let start = 1_793_714_700_000;
        assert_eq!(
            span_text(start, start + 85 * 60_000, chrono_tz::UTC),
            "3 Nov 14:05 to 15:30"
        );
        assert_eq!(
            span_text(start, start + 615 * 60_000, chrono_tz::UTC),
            "3 Nov 14:05 to 4 Nov 00:20"
        );
    }

    // One account's readings never band another account's matches.
    #[test]
    fn match_bands_are_pinned_per_account() {
        let (_dir, conn) = db("rank-bands-per-account");
        let a = arena_account(&conn);
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qsecond#54321', 0, 'arena', 'Qsecond-id')",
            [],
        )
        .expect("second account");
        let b = conn.last_insert_rowid();
        // All in January 1970 UTC (`observe` reads at offset * 1000 ms).
        observe(&conn, a, "constructed", 100, "Gold", 2, 0, 0, 0);
        ranked(&conn, a, "q-a", None, Some(200), "win", 2_000_000);
        ranked(&conn, b, "q-b", None, Some(300), "win", 3_000_000);
        let bands = match_bands_of(&conn, None, chrono_tz::UTC).expect("bands");
        assert_eq!(bands.get("q-a"), Some(&RankBand::Gold));
        assert_eq!(bands.get("q-b"), Some(&RankBand::Unranked));
        let only_b = match_bands_of(&conn, Some(b), chrono_tz::UTC).expect("b");
        assert_eq!(only_b.get("q-b"), Some(&RankBand::Unranked));
        assert_eq!(only_b.get("q-a"), None);
    }

    fn set_kind(conn: &Connection, token: &str, kind: &str) {
        conn.execute(
            "UPDATE matches SET game_structure = ?2 WHERE token = ?1",
            store::params![token, kind],
        )
        .expect("kind");
    }

    #[test]
    fn a_gap_of_one_bo1_match_is_bo1() {
        let (_dir, conn) = db("rank-gap-bo1");
        let (account, _) = seed(&conn);
        set_kind(&conn, "q-a", "bo1");
        set_kind(&conn, "q-b", "bo3");
        let line = rank_line_view(&conn, account, LadderView::Constructed, None)
            .expect("read")
            .expect("a line");
        assert_eq!(line.gaps[0].best_of, Some(BestOfView::Bo1));
        assert_eq!(line.gaps[2].best_of, Some(BestOfView::Bo3));
        assert_eq!(line.matches[0].best_of, Some(BestOfView::Bo1));
        assert_eq!(line.matches[2].best_of, None, "q-c has no kind");
    }

    #[test]
    fn a_gap_mixing_kinds_has_no_kind() {
        let (_dir, conn) = db("rank-gap-mixed");
        let (account, own) = seed(&conn);
        ranked(
            &conn,
            account,
            "q-x",
            Some(own),
            Some(250),
            "win",
            2_500_000,
        );
        set_kind(&conn, "q-a", "bo1");
        set_kind(&conn, "q-x", "bo3");
        let line = rank_line_view(&conn, account, LadderView::Constructed, None)
            .expect("read")
            .expect("a line");
        assert_eq!(line.gaps[0].best_of, None);
        set_kind(&conn, "q-x", "bo1");
        let line = rank_line_view(&conn, account, LadderView::Constructed, None)
            .expect("read")
            .expect("a line");
        assert_eq!(line.gaps[0].best_of, Some(BestOfView::Bo1));
    }

    #[test]
    fn a_played_elsewhere_gap_has_no_kind() {
        let (_dir, conn) = db("rank-gap-elsewhere");
        let (account, _) = seed(&conn);
        set_kind(&conn, "q-a", "bo1");
        let line = rank_line_view(&conn, account, LadderView::Constructed, None)
            .expect("read")
            .expect("a line");
        assert_eq!(line.gaps[1].kind, GapKindView::PlayedElsewhere);
        assert_eq!(line.gaps[1].best_of, None);
    }

    #[test]
    fn the_deck_climb_counts_one_kind() {
        let (_dir, conn) = db("rank-climb-kind");
        let (_, own) = seed(&conn);
        set_kind(&conn, "q-a", "bo1");
        set_kind(&conn, "q-b", "bo3");
        let all = deck_rank_view(&conn, own, None)
            .expect("read")
            .expect("a rank");
        assert_eq!(all.climb.attributed, 2);
        let bo1 = deck_rank_view(&conn, own, Some(store::BestOf::One))
            .expect("read")
            .expect("a rank");
        assert_eq!(bo1.climb.attributed, 1);
        assert_eq!(bo1.climb.text, "4 steps");
        let bo3 = deck_rank_view(&conn, own, Some(store::BestOf::Three))
            .expect("read")
            .expect("a rank");
        assert_eq!(bo3.climb.attributed, 1);
        assert_eq!(bo3.climb.text, "3 steps");
        assert_eq!(bo1.line, all.line, "the line itself is unchanged");
    }

    #[test]
    fn a_manual_claim_and_a_tracked_bo1_match_make_a_shared_gap() {
        let (_dir, conn) = db("rank-shared-bo1");
        let (account, own) = seed(&conn);
        // The played-elsewhere gap (offsets 300 to 500) gains a tracked Bo1
        // match, and the manual match claims it too.
        ranked(
            &conn,
            account,
            "q-t",
            Some(own),
            Some(400),
            "win",
            4_000_000,
        );
        set_kind(&conn, "q-t", "bo1");
        conn.execute(
            "INSERT INTO arena_rank_gap_claims (match_token, arena_log_file_id, byte_offset,
                 claimed_at) VALUES ('q-m', 1, 500, 0)",
            [],
        )
        .expect("claim");
        let line = rank_line_view(&conn, account, LadderView::Constructed, Some(own))
            .expect("read")
            .expect("a line");
        assert_eq!(line.gaps[1].kind, GapKindView::Shared);
        assert!(!line.gaps[1].of_deck, "a shared gap is not attributed");
        let in_gap: Vec<&str> = line
            .matches
            .iter()
            .filter(|m| m.gap == Some(1))
            .map(|m| m.token.as_str())
            .collect();
        assert_eq!(in_gap.len(), 2, "nothing is deleted: {in_gap:?}");
        let claims: i64 = conn
            .query_row("SELECT COUNT(*) FROM arena_rank_gap_claims", [], |r| {
                r.get(0)
            })
            .expect("count");
        assert_eq!(claims, 1);
    }

    #[test]
    fn mythic_text_has_one_decimal() {
        assert_eq!(mythic_text(5.0), "and 5.0% in Mythic");
        assert_eq!(mythic_text(-2.54), "and -2.5% in Mythic");
    }
}
