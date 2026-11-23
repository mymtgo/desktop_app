//! The Endstep source (endstep support spec): folds the stored Endstep
//! capture units into matches, decks and ranked results, through the same
//! match, account, opponent and deck writes as Arena's.
//!
//! The browser extension's helper writes capture units; the app's import
//! worker stores them (`endstep_captures`) and wakes the tracker. First the
//! refs of every unsynced unit are stored, so a fold finds all of a match's
//! units even when a backlog is taken over several polls (#676). Then each
//! poll takes a few unsynced units and folds every match those units belong
//! to from all of its stored units (`fold_match`): a unit arriving late
//! refolds its match whole, so the result is the same in any import order
//! (live equals rebuild). A unit is marked synced only in the batch that
//! writes the last of its matches, and only while its file hash is the one
//! taken (a unit replaced meanwhile is taken again).
//!
//! The work per poll is bounded: at most `REFS_PER_POLL` units have their
//! refs stored (no unit is taken until every unsynced unit has them), at
//! most `UNITS_PER_POLL` units are taken, and at most `GAMES_PER_POLL`
//! matches folded; the rest of a group of units waits in memory for the
//! next poll (`more`), its units still unsynced, so a stop in between takes
//! them again. A captured deck body refolds every
//! match whose lobby names the deck, a few per poll.

use std::collections::{BTreeSet, HashMap, VecDeque};

use endstep_match::{
    DropReason, EndstepDeck, EndstepMatch, Folded, QUIET_MS, account_key, fold_match,
    match_deck_key,
};
use endstep_protocol::{EndstepCardKey, FrameKind, Unit, unit_refs};
use mtgo_match::MatchState;
use store::arena::{PlatformAccount, PlatformOpponent};
use store::decks::DeckWrite;
use store::endstep_captures::decoded_unit;
use store::endstep_sync::{
    ENDSTEP_SYNC_VERSION, EndstepRanked, games_in_start_order, games_of_refs, refs_of_unit,
    sync_version, unfinished_matches, units_of_game, unsynced_unit_hashes,
};
use store::write::{MatchWrite, WriteBatch};
use store::writer::WriterHandle;
use store::{Connection, Platform, StoreError, read};

use super::Engine;
use super::deck_use::{DeckLinks, DeckUseIds, PendingLink, deck_sync_write, is_linked_as};
use crate::TrackerError;

/// The most unsynced units one poll takes.
const UNITS_PER_POLL: u32 = 20;

/// The most matches one poll folds.
const GAMES_PER_POLL: usize = 20;

/// The most unsynced units whose refs one poll stores.
const REFS_PER_POLL: usize = 50;

/// What one poll produced: each match write, with the opponent's display
/// name for an upsert.
#[derive(Debug, Default)]
pub(crate) struct EndstepPoll {
    pub matches: Vec<(MatchWrite, Option<String>)>,
    /// Units or matches are left: poll again without sleeping.
    pub more: bool,
}

/// Units taken whose matches are being folded, a few per poll.
#[derive(Debug, Default)]
struct Group {
    /// The Forge games still to fold.
    games: VecDeque<String>,
    /// The units to mark synced once every game is folded, each with the
    /// file hash of the text taken: a unit replaced since stays unsynced.
    units: Vec<(String, String)>,
}

pub(crate) struct EndstepSource {
    /// Each match's revision, continued from storage, bumped on every write.
    revisions: HashMap<String, u64>,
    /// Deck writes for the next batch and the matches waiting for their
    /// deck links.
    deck_links: DeckLinks,
    /// The units taken and the games still to fold.
    group: Option<Group>,
    /// Units may be unsynced: set at start, when captures are stored and
    /// while a poll takes a full set of units.
    wanted: bool,
    /// A batch with synced marks was sent: the next take waits for it, so
    /// the reader does not see the same units unsynced again.
    marks_sent: bool,
    /// Each match still in progress, as its Forge game and last activity:
    /// folded again once quiet, so it turns abandoned without new captures.
    unfinished: HashMap<String, i64>,
    /// The unsynced units whose refs this run stored, each with the file
    /// hash of the text they were read from.
    refs_stored: HashMap<String, String>,
    /// The games a unit's refs reached before this run replaced them: a
    /// replaced unit's old refs reach the matches it leaves, folded when
    /// the unit is taken.
    left_games: HashMap<String, Vec<String>>,
    /// Tests only: folding this Forge game fails, with a reader SQLite error
    /// when the flag is set (else a deterministic one).
    #[cfg(test)]
    failing_game: Option<(String, bool)>,
}

impl EndstepSource {
    /// Ready to fold, knowing the matches still in progress.
    pub(crate) fn open(reader: &Connection) -> Result<Self, TrackerError> {
        let mut unfinished = HashMap::new();
        for (game, last_activity_ms) in unfinished_matches(reader)? {
            unfinished.insert(game, last_activity_ms);
        }
        Ok(Self {
            revisions: HashMap::new(),
            deck_links: DeckLinks::default(),
            group: None,
            wanted: true,
            marks_sent: false,
            unfinished,
            refs_stored: HashMap::new(),
            left_games: HashMap::new(),
            #[cfg(test)]
            failing_game: None,
        })
    }

    /// Captures were stored: look for unsynced units at the next poll.
    pub(crate) fn wake(&mut self) {
        self.wanted = true;
    }

    /// Fold the matches of up to `UNITS_PER_POLL` unsynced units (or of the
    /// group of units taken before), at most `GAMES_PER_POLL` of them.
    pub(crate) fn poll(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        batch: &mut WriteBatch,
        now_ms: i64,
    ) -> Result<EndstepPoll, TrackerError> {
        let mut out = EndstepPoll::default();
        if self.group.is_none() && self.wanted {
            self.group = self.take_units(reader, writer, now_ms)?;
        }
        if let Some(group) = &mut self.group {
            let count = group.games.len().min(GAMES_PER_POLL);
            let games: Vec<String> = group.games.drain(..count).collect();
            let done = group.games.is_empty();
            for game in games {
                self.fold_game_guarded(reader, writer, batch, &game, now_ms, &mut out)?;
            }
            if done && let Some(group) = self.group.take() {
                // A unit marked synced is not read again; one replaced since
                // has a new hash and has its refs stored again anyway.
                for (unit_id, _) in &group.units {
                    self.refs_stored.remove(unit_id);
                }
                batch.endstep_synced.extend(group.units);
                self.marks_sent = true;
            }
        }
        self.deck_links.write_into(batch);
        out.more = self.group.is_some() || self.wanted;
        Ok(out)
    }

    /// Fold again each match still in progress that has been quiet for
    /// `QUIET_MS`: it turns abandoned.
    pub(crate) fn finalize(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        batch: &mut WriteBatch,
        now_ms: i64,
    ) -> Result<EndstepPoll, TrackerError> {
        let mut out = EndstepPoll::default();
        let mut quiet: Vec<String> = self
            .unfinished
            .iter()
            .filter(|(_, last_activity_ms)| now_ms.saturating_sub(**last_activity_ms) >= QUIET_MS)
            .map(|(game, _)| game.clone())
            .collect();
        quiet.sort();
        for game in quiet {
            self.unfinished.remove(&game);
            self.fold_game_guarded(reader, writer, batch, &game, now_ms, &mut out)?;
        }
        self.deck_links.write_into(batch);
        Ok(out)
    }

    /// Whether a match waits for its deck link: the engine commits the batch
    /// first, then calls `link`.
    pub(crate) fn has_pending_links(&self) -> bool {
        self.deck_links.has_pending_links()
    }

    /// Link each waiting match to its deck version (see `DeckLinks::link`).
    pub(crate) fn link(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
    ) -> Result<(), TrackerError> {
        self.deck_links.link(reader, batch, Platform::Endstep)
    }

    /// The deck ids synced since the last call, for the archetype prefill.
    pub(crate) fn take_written_deck_ids(&mut self) -> Vec<String> {
        self.deck_links.take_written()
    }

    /// Take up to `UNITS_PER_POLL` unsynced units and gather every game
    /// their old and new refs reach, once every unsynced unit has its refs
    /// stored (`store_refs`); until then `None`, with more to do.
    fn take_units(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        now_ms: i64,
    ) -> Result<Option<Group>, TrackerError> {
        if self.marks_sent {
            writer.flush()?;
            self.marks_sent = false;
        }
        // The hashes before the texts: a unit replaced in between is folded
        // from its newer text, and its mark (of the older hash) misses, so
        // it is taken again.
        let mut units = unsynced_unit_hashes(reader)?;
        if units.is_empty() {
            self.wanted = false;
            return Ok(None);
        }
        if !self.store_refs(reader, writer, &units, now_ms)? {
            return Ok(None);
        }
        if units.len() <= UNITS_PER_POLL as usize {
            self.wanted = false;
        }
        units.truncate(UNITS_PER_POLL as usize);
        let mut games: BTreeSet<String> = BTreeSet::new();
        for (unit_id, _) in &units {
            if let Some(left) = self.left_games.remove(unit_id) {
                games.extend(left);
            }
            games.extend(games_of_refs(reader, &refs_of_unit(reader, unit_id)?)?);
        }
        // In play order, so a deck's lists are written oldest first.
        let games = games_in_start_order(reader, games.into_iter().collect())?;
        Ok(Some(Group {
            games: games.into_iter().collect(),
            units,
        }))
    }

    /// Store the refs of up to `REFS_PER_POLL` of the unsynced `units` whose
    /// refs this run has not stored from their current text (in a batch of
    /// their own, committed before any fold reads them). True once every
    /// one of `units` has them.
    ///
    /// The new refs replace the old ones before the fold commit. A stop in
    /// between leaves the units unsynced (taken again) but their old refs
    /// gone, so a match reached only by an old ref is not folded again
    /// (rare: a replaced unit that left its match; a version bump refolds
    /// everything).
    fn store_refs(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        units: &[(String, String)],
        now_ms: i64,
    ) -> Result<bool, TrackerError> {
        let mut missing = Vec::new();
        for (unit_id, file_hash) in units {
            if self.refs_stored.get(unit_id) != Some(file_hash) {
                missing.push((unit_id, file_hash));
            }
        }
        if missing.is_empty() {
            return Ok(true);
        }
        let mut refs_batch = WriteBatch {
            now_ms,
            ..WriteBatch::default()
        };
        for (unit_id, file_hash) in missing.iter().take(REFS_PER_POLL) {
            let left = games_of_refs(reader, &refs_of_unit(reader, unit_id)?)?;
            self.left_games
                .entry((*unit_id).clone())
                .or_default()
                .extend(left);
            let refs = match decoded_unit(reader, unit_id)? {
                Some(unit) => unit_refs(&unit),
                None => Vec::new(),
            };
            refs_batch.endstep_refs.push(((*unit_id).clone(), refs));
            self.refs_stored
                .insert((*unit_id).clone(), (*file_hash).clone());
        }
        writer.write(refs_batch)?;
        writer.flush()?;
        Ok(missing.len() <= REFS_PER_POLL)
    }

    /// `fold_game`, where a deterministic failure of this one match is
    /// logged and skipped (its units are still marked synced, as an
    /// undecodable unit is: a fix and a version bump fold it again), so one
    /// bad match never restarts the tracker over and over. A writer that
    /// failed or stopped, and a SQLite error (busy, IO: transient), are
    /// still errors: the restart, with its backoff, retries the units.
    fn fold_game_guarded(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        batch: &mut WriteBatch,
        game: &str,
        now_ms: i64,
        out: &mut EndstepPoll,
    ) -> Result<(), TrackerError> {
        match self.fold_game(reader, writer, batch, game, now_ms, out) {
            Ok(()) => Ok(()),
            Err(TrackerError::Store(StoreError::Failed(message))) => {
                Err(TrackerError::Store(StoreError::Failed(message)))
            }
            Err(TrackerError::Store(StoreError::WriterGone)) => {
                Err(TrackerError::Store(StoreError::WriterGone))
            }
            Err(TrackerError::Store(StoreError::Sqlite(error))) => {
                Err(TrackerError::Store(StoreError::Sqlite(error)))
            }
            Err(error) => {
                tracing::warn!(game, %error, "an Endstep match could not be folded; skipped");
                Ok(())
            }
        }
    }

    /// Fold the match of the Forge game `game` from all its stored units and
    /// queue what it writes. Nothing is queued unless every step succeeds.
    fn fold_game(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        batch: &mut WriteBatch,
        game: &str,
        now_ms: i64,
        out: &mut EndstepPoll,
    ) -> Result<(), TrackerError> {
        #[cfg(test)]
        if let Some((failing, transient)) = &self.failing_game
            && failing == game
        {
            if *transient {
                let error = reader
                    .execute_batch("SELECT * FROM injected_missing_table")
                    .expect_err("no such table");
                return Err(TrackerError::Store(StoreError::Sqlite(error)));
            }
            return Err(TrackerError::Store(StoreError::Corrupt(
                "injected fold failure".to_string(),
            )));
        }
        let mut units = Vec::new();
        for unit_id in units_of_game(reader, game)? {
            if let Some(unit) = decoded_unit(reader, &unit_id)? {
                units.push(unit);
            }
        }
        match fold_match(game, &units, now_ms) {
            Folded::Match(found) => self.write_match(reader, writer, batch, *found, now_ms, out),
            Folded::Dropped(DropReason::Bot | DropReason::Spectator) => {
                self.unfinished.remove(game);
                if let Some(token) = match_body_id(game, &units)
                    && read::match_revision(reader, &token)?.is_some()
                {
                    self.revisions.remove(&token);
                    out.matches.push((MatchWrite::Remove { token }, None));
                }
                Ok(())
            }
            // A later unit may bring the match body.
            Folded::Dropped(DropReason::NoMatchBody) => Ok(()),
        }
    }

    /// The writes of one folded match: its account and opponent first, then
    /// the match, its deck, its ranked result and its game rows. The steps that can fail
    /// (card ids, deck reads, the revision) run before anything is queued.
    fn write_match(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        batch: &mut WriteBatch,
        found: EndstepMatch,
        now_ms: i64,
        out: &mut EndstepPoll,
    ) -> Result<(), TrackerError> {
        let account = account_key(&found.account.user_id);
        let view = found.view;
        let token = view.token.clone();
        let deck_write = match found.deck {
            Some(deck) => self.deck_write(
                reader,
                writer,
                &token,
                deck,
                &account,
                view.started_at_ms,
                now_ms,
            )?,
            None => None,
        };
        let revision = self.next_revision(reader, &token)?;

        batch.platform_accounts.push(PlatformAccount {
            platform: Platform::Endstep,
            username: account,
            user_id: None,
            display_name: Some(found.account.username.clone()),
            seen_at_ms: now_ms,
        });
        let opponent_name = found
            .opponent
            .as_ref()
            .map(|person| person.username.clone());
        if let Some(person) = &found.opponent {
            batch.platform_opponents.push(PlatformOpponent {
                platform: Platform::Endstep,
                username: account_key(&person.user_id),
                display_name: person.username.clone(),
            });
        }
        if view.state == MatchState::InProgress {
            self.unfinished
                .insert(found.forge_game_id.clone(), view.last_activity_ms);
        } else {
            self.unfinished.remove(&found.forge_game_id);
        }
        out.matches.push((
            MatchWrite::Upsert {
                view: Box::new(view),
                revision,
            },
            opponent_name,
        ));
        if let Some((write, link)) = deck_write {
            self.deck_links.push(write, link);
        }
        if let Some(ranked) = &found.ranked {
            batch
                .endstep_ranked
                .push(EndstepRanked::from_result(&token, ranked));
        }
        batch.endstep_game_cards.push((token, found.game_cards));
        Ok(())
    }

    /// The deck file sync of the deck the match was played with (each card
    /// at its Endstep catalog id) and the link that waits for it; `None`
    /// when the match is linked to that version already.
    #[allow(clippy::too_many_arguments)]
    fn deck_write(
        &mut self,
        reader: &Connection,
        writer: &WriterHandle,
        token: &str,
        deck: EndstepDeck,
        account: &str,
        at_ms: i64,
        now_ms: i64,
    ) -> Result<Option<(DeckWrite, PendingLink)>, TrackerError> {
        let mut keys: Vec<EndstepCardKey> = Vec::new();
        for (key, _) in deck.main.iter().chain(deck.side.iter()) {
            keys.push(key.clone());
        }
        let ids = writer.endstep_card_ids(keys, now_ms)?;
        let mut main = Vec::new();
        let mut side = Vec::new();
        for (index, (_, count)) in deck.main.iter().chain(deck.side.iter()).enumerate() {
            let Some(catalog_id) = ids.get(index) else {
                continue;
            };
            if index < deck.main.len() {
                main.push((*catalog_id, *count));
            } else {
                side.push((*catalog_id, *count));
            }
        }
        let deck_use = DeckUseIds {
            match_token: token.to_string(),
            deck_id: deck.key,
            deck_name: deck.name,
            format: deck.format,
            main,
            side,
        };
        let (write, mut link) =
            deck_sync_write(reader, deck_use, account, at_ms, now_ms, Platform::Endstep)?;
        // A match folded before its deck body was captured had its own deck.
        let own_deck = match_deck_key(token);
        if link.deck_id != own_deck {
            link.leftover = Some(own_deck);
        }
        if is_linked_as(reader, &link)? {
            return Ok(None);
        }
        Ok(Some((write, link)))
    }

    /// The next revision of the match `token`, continued from storage.
    fn next_revision(&mut self, reader: &Connection, token: &str) -> Result<u64, TrackerError> {
        let current = match self.revisions.get(token) {
            Some(revision) => *revision,
            None => read::match_revision(reader, token)?.unwrap_or(0),
        };
        let next = current + 1;
        self.revisions.insert(token.to_string(), next);
        Ok(next)
    }
}

/// The Endstep match id of the last match body of the Forge game `game`, in
/// fold order (the body the fold reads).
fn match_body_id(game: &str, units: &[Unit]) -> Option<String> {
    let mut ordered: Vec<&Unit> = units.iter().collect();
    ordered.sort_by(|a, b| {
        (a.started_at, a.counter, &a.unit_id).cmp(&(b.started_at, b.counter, &b.unit_id))
    });
    let mut id = None;
    for unit in ordered {
        for frame in &unit.frames {
            if let FrameKind::MatchBody(body) = &frame.kind
                && body.forge_game_id == game
            {
                id = Some(body.id.clone());
            }
        }
    }
    id
}

impl Engine {
    /// At start, when the stored Endstep sync version is older than
    /// `ENDSTEP_SYNC_VERSION` (or none is stored), unsync every capture and
    /// store the version, so every Endstep match is folded again (MTGO's
    /// `PROJECTION_VERSION` pattern). Committed before the first poll reads
    /// the units.
    pub(super) fn plan_endstep_resync(&mut self) -> Result<(), TrackerError> {
        let stored = sync_version(&self.reader)?;
        if stored.is_some_and(|version| version >= ENDSTEP_SYNC_VERSION) {
            return Ok(());
        }
        self.writer.write(WriteBatch {
            endstep_resync: true,
            now_ms: self.batch.now_ms,
            ..WriteBatch::default()
        })?;
        self.writer.flush()?;
        Ok(())
    }

    /// Push and queue each Endstep match write: an upsert is pushed with its
    /// platform and opponent name (one the player removed from stats is
    /// stored, never shown), a removal is told.
    pub(super) fn take_endstep_writes(&mut self, writes: Vec<(MatchWrite, Option<String>)>) {
        for (write, opponent_name) in writes {
            match &write {
                MatchWrite::Upsert { view, revision } => {
                    if !self.hidden.contains(&view.token) {
                        self.push_platform_view(
                            view.as_ref().clone(),
                            *revision,
                            Platform::Endstep,
                            opponent_name,
                        );
                    }
                }
                MatchWrite::Remove { token } => {
                    self.pending_events.push(crate::TrackerEvent::MatchRemoved {
                        token: token.clone(),
                    });
                }
                _ => {}
            }
            self.batch.push_match(write, Platform::Endstep);
        }
    }
}

#[cfg(test)]
mod tests {
    use fixture_tools::TestDir;
    use sha2::{Digest, Sha256};
    use store::Connection;
    use store::endstep_captures::NewCapture;
    use store::endstep_sync::ENDSTEP_SYNC_VERSION;

    use crate::engine::Engine;
    use crate::engine::test_support::open;
    use crate::{TrackerConfig, TrackerEvent};

    /// After the fixture's last frame by more than the quiet time.
    const NOW: i64 = 1_794_320_000_000;
    const QUIET_MS: i64 = endstep_match::QUIET_MS;
    const UNIT_01: &str =
        include_str!("../../../../fixtures/endstep/ranked-modern-bo3/01-session-a7a2056a.jsonl");
    const UNIT_02: &str =
        include_str!("../../../../fixtures/endstep/ranked-modern-bo3/02-match-329f3d80.jsonl");
    const UNIT_03: &str =
        include_str!("../../../../fixtures/endstep/ranked-modern-bo3/03-session-e3f486f1.jsonl");
    const UNIT_04: &str =
        include_str!("../../../../fixtures/endstep/ranked-modern-bo3/04-match-946164b1.jsonl");
    const UNIT_05: &str =
        include_str!("../../../../fixtures/endstep/ranked-modern-bo3/05-session-1a930f54.jsonl");
    const ALL: [&str; 5] = [UNIT_01, UNIT_02, UNIT_03, UNIT_04, UNIT_05];
    const TOKEN: &str = "00000000-0000-4000-8000-e0000000003a";
    const FORGE_GAME: &str = "00000000-0000-4000-8000-e0000000003e";
    const UNIT_04_ID: &str = "946164b1-d1e2-4f5f-a509-d3569c3543ba";

    fn config(dir: &TestDir, endstep_enabled: bool) -> TrackerConfig {
        TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: None,
            watching: true,
            arena_enabled: true,
            endstep_enabled,
        }
    }

    fn capture(text: &str) -> NewCapture {
        let parsed = endstep_capture::parse_capture(text).expect("a capture");
        let file_hash = Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        NewCapture {
            header: parsed.header,
            frame_count: parsed.frame_count,
            text: text.to_string(),
            file_hash,
        }
    }

    fn store_units(engine: &mut Engine, texts: &[&str], now_ms: i64) {
        let captures = texts.iter().map(|text| capture(text)).collect();
        engine
            .store_endstep_captures(captures, now_ms)
            .expect("store captures");
    }

    /// Tick until nothing is left, then a few more, and commit. Returns the
    /// events pushed and the final `now`.
    fn run(engine: &mut Engine, start_ms: i64) -> (Vec<TrackerEvent>, i64) {
        let mut now = start_ms;
        let mut events = Vec::new();
        for _ in 0..1_000 {
            let tick = engine.tick(now).expect("tick");
            events.extend(tick.events);
            now += 1;
            if !tick.more {
                break;
            }
        }
        for _ in 0..3 {
            now += 1_000;
            events.extend(engine.tick(now).expect("tick").events);
        }
        engine.flush().expect("flush");
        (events, now)
    }

    fn reader(config: &TrackerConfig) -> Connection {
        store::open_reader(&config.db_path).expect("reader")
    }

    fn number(config: &TrackerConfig, sql: &str) -> i64 {
        reader(config)
            .query_row(sql, [], |row| row.get(0))
            .expect(sql)
    }

    fn texts(config: &TrackerConfig, sql: &str) -> Vec<String> {
        let conn = reader(config);
        let mut stmt = conn.prepare(sql).expect("prepare");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.collect::<Result<_, _>>().expect("rows")
    }

    /// Every row of `table` as text, every column but `skip`, sorted.
    fn dump(config: &TrackerConfig, table: &str, skip: &[&str]) -> Vec<String> {
        let columns: Vec<String> = texts(
            config,
            &format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid"),
        )
        .into_iter()
        .filter(|name| !skip.contains(&name.as_str()))
        .map(|name| format!("quote(\"{name}\")"))
        .collect();
        let mut rows = texts(
            config,
            &format!("SELECT {} FROM {table}", columns.join(" || '|' || ")),
        );
        rows.sort();
        rows
    }

    /// The rows an Endstep sync writes, without the revision (a refold of
    /// the same units is a new revision of the same row) and the game sync
    /// stamp's time (wall clock: only whether it is set). The accounts,
    /// opponents and decks too, without their first-seen and created times
    /// (wall clock).
    fn dumps(config: &TrackerConfig) -> Vec<Vec<String>> {
        vec![
            dump(config, "matches", &["revision", "games_synced_at"]),
            texts(
                config,
                "SELECT token || '|' || (games_synced_at IS NOT NULL) FROM matches ORDER BY token",
            ),
            dump(config, "games", &[]),
            dump(config, "endstep_ranked_results", &[]),
            dump(config, "accounts", &["first_seen_at"]),
            dump(config, "opponents", &["first_seen_at"]),
            dump(config, "decks", &["created_at"]),
            dump(config, "deck_versions", &[]),
            dump(config, "game_cards", &[]),
            dump(config, "game_mulligan_cards", &[]),
        ]
    }

    /// A tracker that synced every fixture unit.
    fn synced_fixture(label: &str) -> (TestDir, TrackerConfig, Vec<TrackerEvent>, i64) {
        let dir = TestDir::new(label);
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        store_units(&mut engine, &ALL, NOW);
        let (events, now) = run(&mut engine, NOW);
        engine.finish().expect("finish");
        (dir, config, events, now)
    }

    #[test]
    fn the_fixture_becomes_one_match() {
        let (_dir, config, events, _) = synced_fixture("endstep-fixture");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(
            texts(
                &config,
                "SELECT m.token || '|' || a.platform || '|' || a.display_name || '|'
                     || o.username || '|' || o.display_name || '|' || m.format || '|'
                     || m.outcome
                 FROM matches m JOIN accounts a ON a.id = m.account_id
                 JOIN opponents o ON o.id = m.opponent_id"
            ),
            vec![format!(
                "{TOKEN}|endstep|Player|endstep:00000000-0000-4000-8000-00000000000b|Opponent|Modern|loss"
            )]
        );
        assert_eq!(
            texts(&config, "SELECT result FROM games ORDER BY game_number"),
            vec!["win", "loss", "loss"]
        );

        let decks = texts(
            &config,
            "SELECT d.name || '|' || v.signature FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id JOIN decks d ON d.id = v.deck_id",
        );
        assert_eq!(decks.len(), 1, "the match is linked to its deck");
        let (name, signature) = decks[0].split_once('|').expect("name and signature");
        assert_eq!(name, "Tron");
        let cards = decks::signature::decode_signature(signature);
        let main: u32 = cards
            .iter()
            .filter(|c| !c.sideboard)
            .map(|c| c.quantity)
            .sum();
        let side: u32 = cards
            .iter()
            .filter(|c| c.sideboard)
            .map(|c| c.quantity)
            .sum();
        assert_eq!((main, side), (60, 15));
        for card in &cards {
            let named = number(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM cards WHERE catalog_id = {} AND name IS NOT NULL",
                    card.catalog_id
                ),
            );
            assert_eq!(named, 1, "card {} has a name", card.catalog_id);
        }

        assert_eq!(
            number(&config, "SELECT rating_after FROM endstep_ranked_results"),
            1336
        );
        assert_eq!(
            number(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM endstep_captures WHERE synced_version = {ENDSTEP_SYNC_VERSION}"
                )
            ),
            5
        );

        let pushed: Vec<(store::Platform, Option<String>)> = events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::MatchUpdated {
                    view,
                    platform,
                    opponent_name,
                    ..
                } if view.token == TOKEN => Some((*platform, opponent_name.clone())),
                _ => None,
            })
            .collect();
        assert!(!pushed.is_empty(), "the match is pushed");
        for push in pushed {
            assert_eq!(
                push,
                (store::Platform::Endstep, Some("Opponent".to_string()))
            );
        }
    }

    /// A hidden Endstep match shown again is pushed live as stored, on
    /// Endstep, with its opponent's shown name (#546).
    #[test]
    fn an_endstep_match_shown_again_is_pushed() {
        use store::match_edits::{MatchEdit, MatchEditOutcome};
        let (_dir, config, _, now) = synced_fixture("endstep-shown-again");
        let mut engine = open(&config, now);
        let (_, now) = run(&mut engine, now);
        let outcome = engine
            .edit_match(MatchEdit::Hide {
                token: TOKEN.to_string(),
                now_ms: now,
            })
            .expect("hide");
        assert_eq!(outcome, MatchEditOutcome::Done);
        engine.tick(now).expect("tick");
        let outcome = engine
            .edit_match(MatchEdit::Restore {
                token: TOKEN.to_string(),
                now_ms: now,
            })
            .expect("restore");
        assert_eq!(outcome, MatchEditOutcome::Done);
        let tick = engine.tick(now + 1).expect("tick");
        let pushed: Vec<(String, store::Platform, bool, Option<String>)> = tick
            .events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::MatchUpdated {
                    view,
                    platform,
                    manual,
                    opponent_name,
                    ..
                } => Some((
                    view.token.clone(),
                    *platform,
                    *manual,
                    opponent_name.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            pushed,
            vec![(
                TOKEN.to_string(),
                store::Platform::Endstep,
                false,
                Some("Opponent".to_string())
            )]
        );
        engine.finish().expect("finish");
    }

    #[test]
    fn import_order_does_not_change_the_match() {
        let (_dir, live, _, _) = synced_fixture("endstep-order-all");
        let dir = TestDir::new("endstep-order-split");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        store_units(&mut engine, &[UNIT_04], NOW);
        let (_, now) = run(&mut engine, NOW);
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 0);
        store_units(&mut engine, &[UNIT_01, UNIT_02, UNIT_03, UNIT_05], now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(dumps(&config), dumps(&live));
    }

    #[test]
    fn rebuild_equals_live() {
        let (_dir, config, _, now) = synced_fixture("endstep-rebuild");
        let live = dumps(&config);
        let revision = number(&config, "SELECT revision FROM matches");
        let writer = store::open_writer(&config.db_path).expect("open");
        writer
            .execute(
                "UPDATE derivations SET version = 0 WHERE name = 'endstep_sync'",
                [],
            )
            .expect("an older version");
        drop(writer);

        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(dumps(&config), live);
        assert!(
            number(&config, "SELECT revision FROM matches") > revision,
            "the match was folded again"
        );
        assert_eq!(
            number(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM endstep_captures WHERE synced_version = {ENDSTEP_SYNC_VERSION}"
                )
            ),
            5
        );
    }

    /// Unit 04 as the extension cuts it when the match ends before its game
    /// socket closes: the match unit up to the lobby `GAME_OVER`, the frames
    /// after it in a session unit.
    fn split_at_match_end() -> (String, String) {
        let lines: Vec<&str> = UNIT_04.lines().collect();
        let end = lines
            .iter()
            .position(|line| line.contains(r#""type":"GAME_OVER""#))
            .expect("a GAME_OVER frame");
        let head = lines[..=end].join("\n") + "\n";
        let first_at: i64 = serde_json::from_str::<serde_json::Value>(lines[end + 1])
            .expect("a frame")["at"]
            .as_i64()
            .expect("at");
        let header = format!(
            r#"{{"mymtgo_endstep_capture":1,"unit_id":"0b1e6a3c-6f7d-4c2e-9a51-7c0d2f4e8b19","kind":"session","started_at":{first_at},"counter":6,"extension":"0.1.0","browser":"chrome","developer":false,"classification":"unknown"}}"#
        );
        let tail = [header.as_str()]
            .into_iter()
            .chain(lines[end + 1..].iter().copied())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        (head, tail)
    }

    #[test]
    fn a_match_unit_closed_at_the_match_end_completes_the_match() {
        let (_dir, live, _, _) = synced_fixture("endstep-end-whole");
        let (head, tail) = split_at_match_end();
        let dir = TestDir::new("endstep-end-split");
        let config = config(&dir, true);
        let just_after = 1_794_311_726_000;
        let mut engine = open(&config, just_after);
        store_units(&mut engine, &[UNIT_01, UNIT_02, UNIT_03, &head], just_after);
        let (_, now) = run(&mut engine, just_after);
        assert_eq!(
            state(&config),
            "complete",
            "complete before the socket closes"
        );
        store_units(&mut engine, &[&tail, UNIT_05], now);
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(dumps(&config), dumps(&live));
    }

    #[test]
    fn a_late_bot_flag_removes_the_match() {
        let (_dir, config, _, now) = synced_fixture("endstep-late-bot");
        let body = UNIT_01
            .lines()
            .find(|line| line.contains("\"forgeGameId\""))
            .expect("a match body line")
            .replacen(
                "\"participants\":[{",
                "\"participants\":[{\"isBot\":true,",
                1,
            );
        let extra = format!(
            "{}\n{body}\n",
            r#"{"mymtgo_endstep_capture":1,"unit_id":"00000000-0000-4000-8000-0000000000f1","kind":"session","started_at":1794311800000,"counter":6,"extension":"0.1.0","browser":"chrome","developer":false,"classification":"unknown"}"#
        );
        let mut engine = open(&config, now);
        store_units(&mut engine, &[&extra], now);
        let (events, _) = run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(
            number(&config, "SELECT COUNT(*) FROM endstep_ranked_results"),
            0
        );
        assert!(
            events.iter().any(|event| matches!(
                event,
                TrackerEvent::MatchRemoved { token } if token == TOKEN
            )),
            "the removal is pushed"
        );
    }

    #[test]
    fn a_switched_off_engine_leaves_captures_unsynced() {
        let dir = TestDir::new("endstep-off");
        let config = config(&dir, false);
        let mut engine = open(&config, NOW);
        store_units(&mut engine, &ALL, NOW);
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM endstep_captures WHERE synced_version = 0"
            ),
            5
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 0);
    }

    /// Before the match ended: units 01 to 03 hold game 1 only.
    const DURING: i64 = 1_794_311_300_000;

    fn state(config: &TrackerConfig) -> String {
        texts(config, "SELECT state FROM matches")
            .pop()
            .unwrap_or_default()
    }

    #[test]
    fn a_quiet_match_turns_abandoned_without_new_captures() {
        let dir = TestDir::new("endstep-quiet-live");
        let config = config(&dir, true);
        let mut engine = open(&config, DURING);
        store_units(&mut engine, &[UNIT_01, UNIT_02, UNIT_03], DURING);
        run(&mut engine, DURING);
        assert_eq!(state(&config), "in_progress");
        run(&mut engine, DURING + QUIET_MS);
        engine.finish().expect("finish");
        assert_eq!(state(&config), "abandoned");
    }

    #[test]
    fn a_quiet_match_turns_abandoned_after_a_restart() {
        let dir = TestDir::new("endstep-quiet-restart");
        let config = config(&dir, true);
        let mut engine = open(&config, DURING);
        store_units(&mut engine, &[UNIT_01, UNIT_02, UNIT_03], DURING);
        run(&mut engine, DURING);
        engine.finish().expect("finish");
        assert_eq!(state(&config), "in_progress");
        let later = DURING + QUIET_MS;
        let mut engine = open(&config, later);
        run(&mut engine, later);
        engine.finish().expect("finish");
        assert_eq!(state(&config), "abandoned");
    }

    fn synced_version(config: &TrackerConfig, unit_id: &str) -> i64 {
        number(
            config,
            &format!("SELECT synced_version FROM endstep_captures WHERE unit_id = '{unit_id}'"),
        )
    }

    /// A session unit with `count` match bodies of other Forge games (no
    /// participants: each folds to a spectated match, writing nothing).
    fn many_games_unit(count: usize) -> String {
        let mut text = r#"{"mymtgo_endstep_capture":1,"unit_id":"00000000-0000-4000-8000-0000000000f2","kind":"session","started_at":1794311900000,"counter":7,"extension":"0.1.0","browser":"chrome","developer":false,"classification":"unknown"}"#.to_string();
        text.push('\n');
        for index in 0..count {
            text.push_str(&format!(
                r#"{{"at":1794311900001,"src":"rest","dir":"in","url":"/api/matches/q-m{index:02}","body":{{"id":"q-m{index:02}","forgeGameId":"q-g{index:02}"}}}}"#
            ));
            text.push('\n');
        }
        text
    }

    #[test]
    fn a_unit_replaced_while_its_group_is_folded_is_folded_again() {
        let dir = TestDir::new("endstep-replaced-mid-group");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        // Unit 04 cut short: no ranked result, games 2 and 3 missing.
        let mut short_04: String = UNIT_04.lines().take(300).collect::<Vec<_>>().join("\n");
        short_04.push('\n');
        let many = many_games_unit(21);
        store_units(
            &mut engine,
            &[UNIT_01, UNIT_02, UNIT_03, &short_04, UNIT_05, &many],
            NOW,
        );
        // One group of 22 games: the fixture's first, then 19 others; the
        // last 2 at the next poll.
        let first = engine.tick(NOW).expect("tick");
        assert!(first.more, "the group needs another poll");
        store_units(&mut engine, &[UNIT_04], NOW + 1);
        engine.tick(NOW + 2).expect("tick");
        engine.flush().expect("flush");
        assert_eq!(
            synced_version(&config, UNIT_04_ID),
            0,
            "replaced after it was taken: its new text is not folded yet"
        );
        assert_eq!(
            synced_version(&config, "a7a2056a-63ea-47e9-a198-028e1ebf8532"),
            ENDSTEP_SYNC_VERSION
        );

        run(&mut engine, NOW + 3);
        engine.finish().expect("finish");
        assert_eq!(synced_version(&config, UNIT_04_ID), ENDSTEP_SYNC_VERSION);
        assert_eq!(
            number(&config, "SELECT rating_after FROM endstep_ranked_results"),
            1336
        );
        assert_eq!(number(&config, "SELECT COUNT(*) FROM games"), 3);
    }

    /// A session unit of its own (`index` picks its id and counter) between
    /// units 02 and 03, holding one frame no match reads.
    fn filler_unit(index: usize) -> String {
        format!(
            "{}\n{}\n",
            format_args!(
                r#"{{"mymtgo_endstep_capture":1,"unit_id":"00000000-0000-4000-8000-0000000001{index:02}","kind":"session","started_at":1794311000000,"counter":{},"extension":"0.1.0","browser":"chrome","developer":false,"classification":"unknown"}}"#,
                100 + index
            ),
            r#"{"at":1794311000001,"src":"rest","dir":"in","url":"/api/other","body":{}}"#
        )
    }

    /// #676: a backlog larger than one poll's units folds each match from
    /// all of its units, never from the part one poll took.
    #[test]
    fn a_backlog_folds_a_match_from_all_its_units() {
        let dir = TestDir::new("endstep-backlog-whole");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        // Units 01, 02 and the first 18 fillers fill the first take of 20;
        // units 03 to 05 (games 2 and 3, the end, the ranked result) come
        // after the 58 fillers. 63 units need two polls of refs.
        let fillers: Vec<String> = (0..58).map(filler_unit).collect();
        let mut texts: Vec<&str> = vec![UNIT_01, UNIT_02];
        texts.extend(fillers.iter().map(String::as_str));
        texts.extend([UNIT_03, UNIT_04, UNIT_05]);
        store_units(&mut engine, &texts, NOW);
        let mut first_state = None;
        for step in 0..100 {
            let tick = engine.tick(NOW + step).expect("tick");
            engine.flush().expect("flush");
            if number(&config, "SELECT COUNT(*) FROM matches") > 0 {
                first_state = Some(state(&config));
                break;
            }
            if !tick.more {
                break;
            }
        }
        assert_eq!(first_state.as_deref(), Some("complete"));
        run(&mut engine, NOW + 100);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM games"), 3);
        assert_eq!(
            number(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM endstep_captures WHERE synced_version = {ENDSTEP_SYNC_VERSION}"
                )
            ),
            63
        );
    }

    #[test]
    fn a_match_that_fails_to_fold_is_skipped() {
        let dir = TestDir::new("endstep-fold-failure");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        engine.endstep.failing_game = Some((FORGE_GAME.to_string(), false));
        store_units(&mut engine, &ALL, NOW);
        // `run` fails the test if a tick errors.
        run(&mut engine, NOW);
        engine.finish().expect("finish");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(
            number(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM endstep_captures WHERE synced_version = {ENDSTEP_SYNC_VERSION}"
                )
            ),
            5,
            "marked synced, as an undecodable unit is"
        );
    }

    /// A SQLite error while folding (busy, IO) is transient: the tick fails,
    /// so the supervisor restarts with backoff, and no unit is marked synced.
    #[test]
    fn a_sqlite_error_while_folding_is_not_skipped() {
        let dir = TestDir::new("endstep-fold-sqlite");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        engine.endstep.failing_game = Some((FORGE_GAME.to_string(), true));
        store_units(&mut engine, &ALL, NOW);
        let mut failed = None;
        for step in 0..100 {
            match engine.tick(NOW + step) {
                Ok(tick) if tick.more => {}
                Ok(_) => break,
                Err(error) => {
                    failed = Some(error.to_string());
                    break;
                }
            }
        }
        let failed = failed.expect("a tick fails");
        assert!(failed.contains("injected_missing_table"), "{failed}");
        drop(engine);
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 0);
        assert_eq!(
            number(
                &config,
                &format!(
                    "SELECT COUNT(*) FROM endstep_captures WHERE synced_version = {ENDSTEP_SYNC_VERSION}"
                )
            ),
            0,
            "the restart takes the units again"
        );
    }

    /// An MTGO projection version bump keeps the Endstep matches: they are
    /// not MTGO's to rebuild (final review I1).
    #[test]
    fn a_projection_bump_keeps_endstep_matches() {
        let (_dir, config, _, now) = synced_fixture("endstep-projection-bump");
        assert_eq!(number(&config, "SELECT COUNT(*) FROM matches"), 1);
        let writer = store::open_writer(&config.db_path).expect("open");
        writer
            .execute_batch(
                "UPDATE derivations SET version = version - 1 WHERE name = 'match_projection';
                 UPDATE matches SET derived_version = derived_version - 1;",
            )
            .expect("older projection");
        drop(writer);
        let mut engine = open(&config, now);
        let (events, now) = run(&mut engine, now);
        engine.finish().expect("finish");
        let removed = events
            .iter()
            .filter(|event| matches!(event, TrackerEvent::MatchRemoved { .. }))
            .count();
        let after_bump = number(&config, "SELECT COUNT(*) FROM matches");
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        let after_restart = number(&config, "SELECT COUNT(*) FROM matches");
        assert_eq!(
            (after_bump, after_restart, removed),
            (1, 1, 0),
            "(matches after bump, after restart, MatchRemoved events)"
        );
    }

    /// A second match of the same deck, played a day later with an edited
    /// list: the fixture's units with other ids.
    fn second_match(text: &str) -> String {
        text.replace(
            "00000000-0000-4000-8000-e0000000003e",
            "10000000-0000-4000-8000-e0000000003e",
        )
        .replace(
            "00000000-0000-4000-8000-e0000000003a",
            "10000000-0000-4000-8000-e0000000003a",
        )
        .replace(
            "00000000-0000-4000-8000-e0000000003d",
            "10000000-0000-4000-8000-e0000000003d",
        )
        .replace(
            "00000000-0000-4000-8000-e00000000038",
            "10000000-0000-4000-8000-e00000000038",
        )
        .replace(
            "a7a2056a-63ea-47e9-a198-028e1ebf8532",
            "b7a2056a-63ea-47e9-a198-028e1ebf8532",
        )
        .replace(
            "329f3d80-56d9-4d83-a482-3c888f11fe9f",
            "429f3d80-56d9-4d83-a482-3c888f11fe9f",
        )
        .replace(
            "e3f486f1-ab7a-411a-971c-671216111d2a",
            "f3f486f1-ab7a-411a-971c-671216111d2a",
        )
        .replace(
            "946164b1-d1e2-4f5f-a509-d3569c3543ba",
            "a46164b1-d1e2-4f5f-a509-d3569c3543ba",
        )
        .replace(
            "1a930f54-5d1a-497d-ac0f-de5b32ed0d36",
            "2a930f54-5d1a-497d-ac0f-de5b32ed0d36",
        )
        .replace("\"started_at\":17943", "\"started_at\":17953")
        .replace("2026-11-10T11:41:49.543Z", "2026-11-11T11:41:49.543Z")
        .replace(
            "{\"name\":\"Dismember\",\"quantity\":2}",
            "{\"name\":\"Dismember\",\"quantity\":3}",
        )
    }

    /// Each match's own deck version, as `token|signature`, by start.
    fn linked_signatures(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT m.token || '|' || COALESCE(v.signature, 'NULL') FROM matches m
             LEFT JOIN deck_versions v ON v.id = m.deck_version_id ORDER BY m.started_at",
        )
    }

    /// #690: an Endstep deck the sync wrote without an archetype is told to
    /// the archetypes worker, as MTGO's deck files are.
    #[test]
    fn a_synced_endstep_deck_asks_for_an_archetype() {
        let dir = TestDir::new("endstep-deck-needs-archetype");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        store_units(&mut engine, &ALL, NOW);
        let (events, _) = run(&mut engine, NOW);
        engine.finish().expect("finish");
        let asked: Vec<i64> = events
            .iter()
            .filter_map(|event| match event {
                TrackerEvent::DeckNeedsArchetype { deck_ids } => Some(deck_ids.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let decks: Vec<i64> = texts(&config, "SELECT CAST(id AS TEXT) FROM decks")
            .iter()
            .map(|id| id.parse().expect("id"))
            .collect();
        assert!(!decks.is_empty(), "the match wrote its deck");
        assert_eq!(asked, decks);
    }

    /// Two matches of one deck with different lists link to their own deck
    /// versions whatever order they are folded in (final review I2).
    #[test]
    fn a_backlog_links_each_match_to_its_own_deck_version() {
        let second: Vec<String> = ALL.iter().map(|text| second_match(text)).collect();
        assert!(
            second[0].contains("\"Dismember\",\"quantity\":3"),
            "the list was edited"
        );
        let second_refs: Vec<&str> = second.iter().map(String::as_str).collect();

        // In play order, one match at a time.
        let dir = TestDir::new("endstep-decks-ordered");
        let ordered = config(&dir, true);
        let mut engine = open(&ordered, NOW);
        store_units(&mut engine, &ALL, NOW);
        let (_, now) = run(&mut engine, NOW);
        store_units(&mut engine, &second_refs, now);
        run(&mut engine, now);
        engine.finish().expect("finish");

        // As a backlog: every unit stored before the first fold.
        let dir = TestDir::new("endstep-decks-backlog");
        let backlog = config(&dir, true);
        let mut engine = open(&backlog, NOW);
        let mut all: Vec<&str> = ALL.to_vec();
        all.extend(second_refs.iter().copied());
        store_units(&mut engine, &all, NOW);
        run(&mut engine, NOW);
        engine.finish().expect("finish");

        // Second match first, then the first match: the older list arrives late.
        let dir = TestDir::new("endstep-decks-late");
        let late = config(&dir, true);
        let mut engine = open(&late, NOW);
        store_units(&mut engine, &second_refs, NOW);
        let (_, now) = run(&mut engine, NOW);
        store_units(&mut engine, &ALL, now);
        run(&mut engine, now);
        engine.finish().expect("finish");

        for config in [&ordered, &backlog, &late] {
            assert_eq!(
                number(
                    config,
                    "SELECT COUNT(*) FROM matches WHERE deck_version_id IS NULL"
                ),
                0,
                "every match is linked"
            );
            assert_eq!(number(config, "SELECT COUNT(*) FROM deck_versions"), 2);
            let links = linked_signatures(config);
            assert_ne!(links[0].split('|').nth(1), links[1].split('|').nth(1));
        }
        assert_eq!(linked_signatures(&backlog), linked_signatures(&ordered));
        assert_eq!(linked_signatures(&late), linked_signatures(&ordered));
        // The deck's current list is the newer match's.
        let current = |config: &TrackerConfig| {
            texts(
                config,
                "SELECT name || '|' || COALESCE(file_synced_at, '') FROM decks",
            )
        };
        assert_eq!(current(&late), current(&ordered));
    }

    /// The match's linked deck as `mtgo_id|name`.
    fn linked_deck(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT d.mtgo_id || '|' || d.name FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id JOIN decks d ON d.id = v.deck_id",
        )
    }

    /// The decks as `mtgo_id|archived`, by key.
    fn archived_decks(config: &TrackerConfig) -> Vec<String> {
        texts(
            config,
            "SELECT mtgo_id || '|' || (archived_at IS NOT NULL) FROM decks ORDER BY mtgo_id",
        )
    }

    /// #867: a match folded before its deck body was captured gets its own
    /// "Endstep deck". Once the deck body arrives, the refold links the
    /// match to the real deck and archives the leftover one, and a rebuild
    /// keeps it archived.
    #[test]
    fn a_leftover_match_deck_is_archived_after_the_refold() {
        let dir = TestDir::new("endstep-leftover-deck");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        let without_deck: String = UNIT_01
            .lines()
            .filter(|line| !line.contains(r#""url":"/api/decks/"#))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_ne!(without_deck, UNIT_01, "the deck bodies are cut");
        store_units(
            &mut engine,
            &[&without_deck, UNIT_02, UNIT_03, UNIT_04, UNIT_05],
            NOW,
        );
        let (_, now) = run(&mut engine, NOW);
        let leftover = format!("endstep:match:{TOKEN}");
        assert_eq!(
            linked_deck(&config),
            vec![format!("{leftover}|Endstep deck")]
        );

        store_units(&mut engine, &[UNIT_01], now);
        let (_, now) = run(&mut engine, now);
        engine.finish().expect("finish");
        let real = "endstep:00000000-0000-4000-8000-e00000000031";
        assert_eq!(linked_deck(&config), vec![format!("{real}|Tron")]);
        let archived = vec![format!("{real}|0"), format!("{leftover}|1")];
        assert_eq!(archived_decks(&config), archived);

        // A rebuild folds the match again from every unit: no flip.
        let writer = store::open_writer(&config.db_path).expect("open");
        writer
            .execute(
                "UPDATE derivations SET version = 0 WHERE name = 'endstep_sync'",
                [],
            )
            .expect("an older version");
        drop(writer);
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(linked_deck(&config), vec![format!("{real}|Tron")]);
        assert_eq!(archived_decks(&config), archived);
    }

    /// The columns of `table` but `skip`, in order.
    fn columns(config: &TrackerConfig, table: &str, skip: &[&str]) -> Vec<String> {
        texts(
            config,
            &format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid"),
        )
        .into_iter()
        .filter(|name| !skip.contains(&name.as_str()))
        .collect()
    }

    /// The `game_cards` rows of the match `token` as text, by game number
    /// and card name rather than ids, sorted.
    fn card_rows(config: &TrackerConfig, token: &str) -> Vec<String> {
        let values: Vec<String> = columns(config, "game_cards", &["game_id", "catalog_id"])
            .into_iter()
            .map(|name| format!("quote(gc.\"{name}\")"))
            .collect();
        let mut rows = texts(
            config,
            &format!(
                "SELECT g.game_number || '|' || quote(c.name) || '|' || {}
                 FROM game_cards gc JOIN games g ON g.id = gc.game_id
                 JOIN matches m ON m.id = g.match_id
                 LEFT JOIN cards c ON c.catalog_id = gc.catalog_id
                 WHERE m.token = '{token}'",
                values.join(" || '|' || ")
            ),
        );
        rows.sort();
        rows
    }

    /// The hand size columns of the match `token`'s games and its mulligan
    /// hands as text, by game number and card name, sorted.
    fn hand_rows(config: &TrackerConfig, token: &str) -> Vec<String> {
        let mut rows = texts(
            config,
            &format!(
                "SELECT g.game_number || '|' || quote(g.local_opening_hand_size) || '|'
                     || quote(g.local_starting_hand_size) || '|'
                     || quote(g.opp_opening_hand_size) || '|' || quote(g.opp_starting_hand_size)
                 FROM games g JOIN matches m ON m.id = g.match_id WHERE m.token = '{token}'"
            ),
        );
        rows.extend(texts(
            config,
            &format!(
                "SELECT 'mulligan|' || g.game_number || '|' || mc.hand || '|' || quote(c.name)
                     || '|' || mc.quantity
                 FROM game_mulligan_cards mc JOIN games g ON g.id = mc.game_id
                 JOIN matches m ON m.id = g.match_id
                 LEFT JOIN cards c ON c.catalog_id = mc.catalog_id
                 WHERE m.token = '{token}'"
            ),
        ));
        rows.sort();
        rows
    }

    #[test]
    fn the_fixture_match_has_game_rows() {
        let (_dir, config, _, _) = synced_fixture("endstep-game-rows");
        for game_number in 1..=3 {
            for side in ["local", "opponent"] {
                let count = number(
                    &config,
                    &format!(
                        "SELECT COUNT(*) FROM game_cards gc JOIN games g ON g.id = gc.game_id
                         WHERE g.game_number = {game_number} AND gc.side = '{side}'"
                    ),
                );
                assert!(count > 0, "game {game_number} has {side} rows");
            }
        }
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM game_cards gc LEFT JOIN cards c ON c.catalog_id = gc.catalog_id
                 WHERE c.name IS NULL"
            ),
            0,
            "every row's card has a name"
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM matches WHERE games_synced_at IS NOT NULL"
            ),
            1,
            "stamped for archetype detection"
        );
    }

    #[test]
    fn rows_survive_a_rebuild() {
        let (_dir, config, _, now) = synced_fixture("endstep-rows-rebuild");
        let live = card_rows(&config, TOKEN);
        let live_hands = hand_rows(&config, TOKEN);
        assert!(!live.is_empty());
        assert!(live_hands.iter().any(|row| row.starts_with("mulligan|")));
        let writer = store::open_writer(&config.db_path).expect("open");
        writer
            .execute_batch(&format!(
                "UPDATE derivations SET version = 0 WHERE name = 'endstep_sync';
                 UPDATE matches SET archetypes_detected_at = 5 WHERE token = '{TOKEN}';"
            ))
            .expect("an older version, archetypes detected");
        drop(writer);
        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(card_rows(&config, TOKEN), live);
        assert_eq!(hand_rows(&config, TOKEN), live_hands);
        assert_eq!(
            texts(
                &config,
                &format!(
                    "SELECT quote(archetypes_detected_at) FROM matches WHERE token = '{TOKEN}'"
                )
            ),
            vec!["5"],
            "unchanged rows keep the detection"
        );
    }

    #[test]
    fn import_order_does_not_change_rows() {
        let (_dir, live, _, _) = synced_fixture("endstep-rows-order-all");
        let dir = TestDir::new("endstep-rows-order-reverse");
        let config = config(&dir, true);
        let mut engine = open(&config, NOW);
        let mut now = NOW;
        for text in ALL.iter().rev() {
            store_units(&mut engine, &[*text], now);
            now = run(&mut engine, now).1;
        }
        engine.finish().expect("finish");
        let rows = card_rows(&config, TOKEN);
        assert!(!rows.is_empty());
        assert_eq!(rows, card_rows(&live, TOKEN));
        assert_eq!(hand_rows(&config, TOKEN), hand_rows(&live, TOKEN));
    }

    /// A copy of the Endstep match, its games and its game rows under
    /// `token`, for a new account `username` of `platform`.
    fn copy_match(config: &TrackerConfig, token: &str, username: &str, platform: &str) {
        let match_columns = columns(config, "matches", &["id", "token", "account_id"]).join(", ");
        let game_columns = columns(config, "games", &["id", "match_id"]).join(", ");
        let card_columns = columns(config, "game_cards", &["game_id"]).join(", ");
        let writer = store::open_writer(&config.db_path).expect("open");
        let account_id: i64 = writer
            .query_row(
                "INSERT INTO accounts (username, first_seen_at, platform) VALUES (?1, 0, ?2)
                 RETURNING id",
                [username, platform],
                |row| row.get(0),
            )
            .expect("account");
        let match_id: i64 = writer
            .query_row(
                &format!(
                    "INSERT INTO matches (token, account_id, {match_columns})
                     SELECT ?1, ?2, {match_columns} FROM matches WHERE token = ?3 RETURNING id"
                ),
                store::params![token, account_id, TOKEN],
                |row| row.get(0),
            )
            .expect("match");
        writer
            .execute(
                &format!(
                    "INSERT INTO games (match_id, {game_columns})
                     SELECT ?1, {game_columns} FROM games
                     WHERE match_id = (SELECT id FROM matches WHERE token = ?2)"
                ),
                store::params![match_id, TOKEN],
            )
            .expect("games");
        let copied = card_columns
            .split(", ")
            .map(|name| format!("gc.{name}"))
            .collect::<Vec<_>>()
            .join(", ");
        writer
            .execute(
                &format!(
                    "INSERT INTO game_cards (game_id, {card_columns})
                     SELECT ng.id, {copied} FROM game_cards gc
                     JOIN games og ON og.id = gc.game_id
                     JOIN games ng ON ng.match_id = ?1 AND ng.game_number = og.game_number
                     WHERE og.match_id = (SELECT id FROM matches WHERE token = ?2)"
                ),
                store::params![match_id, TOKEN],
            )
            .expect("game rows");
    }

    /// The other platforms' matches: their game rows, hands and stamps.
    fn other_platforms(config: &TrackerConfig) -> Vec<Vec<String>> {
        vec![
            card_rows(config, "q-mtgo"),
            card_rows(config, "q-arena"),
            hand_rows(config, "q-mtgo"),
            hand_rows(config, "q-arena"),
            texts(
                config,
                "SELECT token || '|' || quote(games_synced_at) || '|' || quote(archetypes_detected_at)
                 FROM matches WHERE token IN ('q-mtgo', 'q-arena') ORDER BY token",
            ),
        ]
    }

    #[test]
    fn the_version_bump_adds_rows_to_old_matches() {
        let (_dir, config, _, now) = synced_fixture("endstep-rows-version-bump");
        copy_match(&config, "q-mtgo", "Qmtgo", "mtgo");
        copy_match(&config, "q-arena", "Qarena", "arena");
        // As a sync at version 1 left it: no game rows, not stamped, and
        // archetypes detected.
        let writer = store::open_writer(&config.db_path).expect("open");
        writer
            .execute_batch(&format!(
                "DELETE FROM game_cards WHERE game_id IN (SELECT g.id FROM games g
                     JOIN matches m ON m.id = g.match_id WHERE m.token = '{TOKEN}');
                 UPDATE matches SET games_synced_at = NULL WHERE token = '{TOKEN}';
                 UPDATE matches SET archetypes_detected_at = 5;
                 UPDATE derivations SET version = 1 WHERE name = 'endstep_sync';
                 UPDATE endstep_captures SET synced_version = 1;"
            ))
            .expect("a version 1 database");
        drop(writer);
        let before = other_platforms(&config);
        assert!(!before[0].is_empty() && !before[1].is_empty());
        assert!(card_rows(&config, TOKEN).is_empty());

        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert!(!card_rows(&config, TOKEN).is_empty(), "the rows are added");
        assert_eq!(
            texts(
                &config,
                &format!(
                    "SELECT quote(archetypes_detected_at) || '|' || (games_synced_at IS NOT NULL)
                     FROM matches WHERE token = '{TOKEN}'"
                )
            ),
            vec!["NULL|1"],
            "stamped and detected again"
        );
        assert_eq!(
            other_platforms(&config),
            before,
            "the other platforms are untouched"
        );
    }

    #[test]
    fn the_fixture_match_has_hands_and_sideboarding() {
        let (_dir, config, _, _) = synced_fixture("endstep-hands");
        let hands = hand_rows(&config, TOKEN);
        assert!(
            hands.iter().any(|row| row.starts_with("1|7|7|")),
            "game 1: the user kept 7 ({hands:?})"
        );
        assert_eq!(
            number(
                &config,
                "SELECT opp_starting_hand_size FROM games WHERE game_number = 1"
            ),
            6,
            "the opponent mulliganed to 6 in game 1"
        );
        assert_eq!(
            number(
                &config,
                "SELECT local_opening_hand_size * 10 + local_starting_hand_size
                 FROM games WHERE game_number = 3"
            ),
            76,
            "game 3: seven drawn, one bottomed"
        );
        assert_eq!(
            number(
                &config,
                "SELECT SUM(quantity) FROM game_mulligan_cards mc
                 JOIN games g ON g.id = mc.game_id
                 WHERE g.game_number = 3 AND mc.hand = 1"
            ),
            7,
            "the hand sent back in game 3"
        );
        assert_eq!(
            number(
                &config,
                "SELECT COUNT(*) FROM game_cards gc JOIN games g ON g.id = gc.game_id
                 WHERE g.game_number = 3 AND gc.side = 'local' AND gc.bottomed = 1"
            ),
            1
        );
        assert_eq!(
            number(
                &config,
                "SELECT gc.sided_out FROM game_cards gc JOIN games g ON g.id = gc.game_id
                 JOIN cards c ON c.catalog_id = gc.catalog_id
                 WHERE g.game_number = 2 AND gc.side = 'local' AND c.name = 'Giant''s Boulder'"
            ),
            2
        );
    }

    #[test]
    fn the_version_bump_fills_hands_on_old_matches() {
        let (_dir, config, _, now) = synced_fixture("endstep-hands-version-bump");
        let live = (card_rows(&config, TOKEN), hand_rows(&config, TOKEN));
        copy_match(&config, "q-mtgo", "Qmtgo", "mtgo");
        copy_match(&config, "q-arena", "Qarena", "arena");
        // As a sync at version 2 left it: rows without hands or decks.
        let writer = store::open_writer(&config.db_path).expect("open");
        writer
            .execute_batch(&format!(
                "UPDATE games SET local_opening_hand_size = 0, local_starting_hand_size = 0,
                     opp_opening_hand_size = 0, opp_starting_hand_size = 0
                 WHERE match_id = (SELECT id FROM matches WHERE token = '{TOKEN}');
                 UPDATE game_cards SET opening_hand = 0, bottomed = 0, starting_hand = 0,
                     sided_in = 0, sided_out = 0
                 WHERE game_id IN (SELECT g.id FROM games g
                     JOIN matches m ON m.id = g.match_id WHERE m.token = '{TOKEN}');
                 DELETE FROM game_mulligan_cards WHERE game_id IN (SELECT g.id FROM games g
                     JOIN matches m ON m.id = g.match_id WHERE m.token = '{TOKEN}');
                 UPDATE derivations SET version = 2 WHERE name = 'endstep_sync';
                 UPDATE endstep_captures SET synced_version = 2;"
            ))
            .expect("a version 2 database");
        drop(writer);
        let before = other_platforms(&config);
        assert!(!before[0].is_empty() && !before[1].is_empty());
        for copied in [&before[2], &before[3]] {
            assert!(
                copied.iter().any(|row| row.starts_with("1|7|7|")),
                "the copies keep their hands: {copied:?}"
            );
        }
        assert_ne!(hand_rows(&config, TOKEN), live.1);

        let mut engine = open(&config, now);
        run(&mut engine, now);
        engine.finish().expect("finish");
        assert_eq!(hand_rows(&config, TOKEN), live.1, "the hands are filled");
        assert_eq!(card_rows(&config, TOKEN), live.0, "the decks are filled");
        assert_eq!(
            other_platforms(&config),
            before,
            "the other platforms are untouched"
        );
    }
}
