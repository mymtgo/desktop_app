//! Linking matches to the deck version played (decks spec 4; 0.x
//! `ResolveMatchDeckFromLog`, `FindDeckVersionByOracleIdentity` and
//! `RelinkOrphanMatches`).
//!
//! When: a visible match with games and no link is tried once the tracker has
//! caught up after it changed; after every deck sync and every 0.x deck
//! import, unlinked matches started within `RELINK_WITHIN_MS` are tried again
//! (at most `RELINK_LIMIT`). After either, an unlinked tournament match takes
//! the one version its other rounds use (at most `SIBLING_LIMIT`).
//!
//! The list played, in order of trust:
//! 1. game 1's `deck_used` list: the deck as edited, before any sideboarding;
//! 2. the registered list (`FlsMatchDeckGetRespMessage`), which MTGO logs
//!    before sideboarding after game 1 and which then equals game 1's list;
//! 3. for a tournament round, the tournament's registered list
//!    (`FlsTournamentDeckRespMessage`), which MTGO logs at a login while the
//!    tournament runs and locks for the whole event. After a late login it is
//!    round 1's only list until sideboarding: game 1 logs `[NULL]`;
//! 4. a later game's `deck_used` list. Sideboarding moves cards between main
//!    and sideboard but keeps each card's total, so only the totals count.
//!
//! A game 1 or registered list (of the match or the tournament) matches a
//! version by exact signature, then (for a live match, once per tick) after
//! reading the deck files again, then by oracle identity. Versions only ever
//! come from deck files. Linking writes only `matches.deck_version_id`: never
//! the projection's columns or the match's revision.

use std::collections::{BTreeMap, HashMap, HashSet};

use decks::{DeckCard, decode_signature, oracle_key, parse_deck_used, parse_registered_deck};
use mtgo_match::MatchView;
use store::StoreError;
use store::compact::{archived_events, compacted_at};
use store::decks::{
    DeckWrite, VersionRow, account_versions, deck_used_raws, link_of, oracle_ids,
    registered_deck_raw, tournament_deck, tournament_sibling_versions, unlinked_matches_since,
};

use super::Engine;
use crate::TrackerError;

/// A match with no deck link that failed for want of a list is tried again at
/// most this often while it keeps changing.
pub const LINK_RETRY_MS: i64 = 2_000;
/// After a deck sync, unlinked matches started this recently are tried again.
pub const RELINK_WITHIN_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// At most this many matches per relink pass (0.x `RelinkOrphanMatches(200)`).
pub const RELINK_LIMIT: u32 = 200;
/// The newest unlinked tournament matches looked at per pass for a version
/// their rounds agree on (0.x `inheritTournamentDecks(50)`).
pub const SIBLING_LIMIT: u32 = 50;
/// Only a match active this recently reads the deck files again on an exact
/// miss: one just played, not one re-derived by a catch-up or a rebuild.
pub const REREAD_ACTIVE_WITHIN_MS: i64 = 60 * 60 * 1000;

/// A match waiting for a link attempt.
#[derive(Debug, Clone)]
struct LinkTarget {
    /// The match's local account: its deck versions are the candidates.
    account: String,
    /// One entry per game, in game order (game 1 first).
    game_ids: Vec<Option<u64>>,
    tournament_event_id: Option<u64>,
    /// The match's last activity (log time).
    active_ms: i64,
}

/// The list a match was played with, and how far it can be trusted.
enum Played {
    /// Game 1's list or a registered list: the deck as edited.
    BeforeSideboarding(Vec<DeckCard>),
    /// A later game's list, possibly sideboarded.
    LaterGame(Vec<DeckCard>),
}

/// What one link attempt did.
#[derive(Debug, PartialEq, Eq)]
enum Attempt {
    Linked,
    /// No list stored for the match yet.
    NoList,
    /// A list, but no version (or more than one) fits it.
    NoVersion,
}

/// The deck lists in one match archive, read once per archive write.
#[derive(Debug)]
struct ArchivedLists {
    compacted_at: i64,
    /// The first parseable `deck_used` list of each Game ID.
    deck_used: HashMap<u64, Vec<DeckCard>>,
    /// The first registered list of the match.
    registered: Option<Vec<DeckCard>>,
}

/// The engine's linking bookkeeping.
#[derive(Debug, Default)]
pub(super) struct LinkState {
    /// Matches to try at the next caught-up tick.
    pending: BTreeMap<String, LinkTarget>,
    /// Matches known to have a link (stored or sent).
    linked: HashSet<String>,
    /// Matches whose list fitted no version: changes to the match cannot help,
    /// only a deck sync can, and the relink pass after it tries them again.
    unmatched: HashSet<String>,
    /// When a match that had no list yet may be tried again.
    retry_at: HashMap<String, i64>,
    /// Matches tried since the decks or the card data last changed that did
    /// not link: a relink pass would give them the same answer, so it skips
    /// them and older matches get their turn.
    relink_skip: HashSet<String>,
    /// A deck sync ran: relink recent matches at the next link phase.
    relink_wanted: bool,
    /// The deck files were read during this tick, so a fresh read on an exact
    /// miss would find nothing new (and catch-up never reads them per match).
    synced_this_tick: bool,
    archives: HashMap<String, ArchivedLists>,
    /// When each match that went through `queue_link` started: the state
    /// above is dropped once a match leaves the relink window, since no
    /// attempt is made for it any more.
    started: HashMap<String, i64>,
    /// Oracle keys of version signatures, by version id (0.x memoises them
    /// too). Only complete keys are kept: missing card data may arrive later.
    oracle_keys: HashMap<i64, (String, String)>,
    /// Each account's deck versions, loaded once per tick and again after the
    /// deck files are read or the decks change, not at every attempt.
    versions: HashMap<String, Vec<VersionRow>>,
    /// Card totals of version signatures, by version id: a signature is
    /// decoded once, not at every later-game attempt.
    totals: HashMap<i64, VersionTotals>,
}

/// A version's signature and the card totals it decodes to.
type VersionTotals = (String, BTreeMap<u32, u64>);

impl LinkState {
    /// A tick starts.
    pub(super) fn new_tick(&mut self) {
        self.synced_this_tick = false;
        self.versions.clear();
    }

    /// The deck files were just read.
    pub(super) fn synced(&mut self) {
        self.synced_this_tick = true;
        self.relink_wanted = true;
        self.versions.clear();
    }

    /// The deck versions, the decks' accounts or the card data changed (a
    /// deck write or a 0.x import): every unlinked match may link now, so
    /// recent ones are tried again at the next link phase.
    pub(super) fn candidates_changed(&mut self) {
        self.relink_skip.clear();
        self.relink_wanted = true;
        self.versions.clear();
    }

    /// The match is gone or failed: no attempt for it, and it no longer
    /// counts as linked. A removed match's row (and so its link, a column on
    /// it) may be deleted, and a link sent in the same batch as the removal
    /// is applied after it and finds no row. If the match derives again, the
    /// link phase reads its stored link first (`link_of`) and links it again
    /// only when it has none.
    pub(super) fn forget(&mut self, token: &str) {
        self.pending.remove(token);
        self.linked.remove(token);
        // The rest is bookkeeping for a match that may derive again: it is
        // made afresh then, and a long session does not accumulate it.
        self.unmatched.remove(token);
        self.retry_at.remove(token);
        self.relink_skip.remove(token);
        self.archives.remove(token);
        self.started.remove(token);
    }

    /// Drop the state of matches started before the relink window: neither the
    /// live path nor a relink pass tries them any more.
    pub(super) fn prune(&mut self, now_ms: i64) {
        let old: Vec<String> = self
            .started
            .iter()
            .filter(|(_, started)| now_ms - **started > RELINK_WITHIN_MS)
            .map(|(token, _)| token.clone())
            .collect();
        for token in old {
            self.forget(&token);
            self.started.remove(&token);
        }
    }

    /// The match linked: its list cache and failure marks are no longer needed.
    fn settled(&mut self, token: &str) {
        self.unmatched.remove(token);
        self.retry_at.remove(token);
        self.relink_skip.remove(token);
        self.archives.remove(token);
    }

    /// The match is known to have a link (stored or sent) this run.
    pub(super) fn is_linked(&self, token: &str) -> bool {
        self.linked.contains(token)
    }
}

impl Engine {
    /// Queue a link attempt for a visible match (from `record_derived`): one
    /// with games, no link and no failed attempt since the last deck sync.
    pub(super) fn queue_link(&mut self, view: &MatchView) {
        let token = &view.token;
        if view.games.is_empty() {
            return;
        }
        // Like the relink pass, only a match started recently: a rebuild must
        // not link what the live path would never retry.
        if self.batch.now_ms - view.started_at_ms > RELINK_WITHIN_MS {
            return;
        }
        self.links.started.insert(token.clone(), view.started_at_ms);
        if self.links.linked.contains(token) || self.links.unmatched.contains(token) {
            return;
        }
        let mut games: Vec<(u32, Option<u64>)> = view
            .games
            .iter()
            .map(|game| (game.game_number, game.mtgo_game_id))
            .collect();
        games.sort_by_key(|(number, _)| *number);
        let game_ids = games.into_iter().map(|(_, id)| id).collect();
        self.links.pending.insert(
            token.clone(),
            LinkTarget {
                account: view.local_username.clone(),
                game_ids,
                tournament_event_id: view.tournament_event_id,
                active_ms: view.last_activity_ms,
            },
        );
    }

    /// Try the queued matches, then relink recent unlinked matches when a deck
    /// sync ran. Runs once the tracker has caught up, so storage holds the
    /// lists of every event read.
    pub(super) fn link_matches(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        self.links.prune(now_ms);
        let mut due: Vec<(String, LinkTarget)> = Vec::new();
        let mut waiting: BTreeMap<String, LinkTarget> = BTreeMap::new();
        for (token, target) in std::mem::take(&mut self.links.pending) {
            let wait = self
                .links
                .retry_at
                .get(&token)
                .is_some_and(|at| now_ms < *at);
            if wait {
                waiting.insert(token, target);
            } else {
                due.push((token, target));
            }
        }
        self.links.pending = waiting;
        if due.is_empty() && !self.links.relink_wanted {
            return Ok(());
        }
        // Storage must hold every event and deck write sent so far.
        self.flush()?;

        for (token, target) in due {
            if self.links.linked.contains(&token) {
                continue;
            }
            if link_of(&self.reader, &token)?.is_some() {
                self.links.linked.insert(token);
                continue;
            }
            let just_played = now_ms - target.active_ms <= REREAD_ACTIVE_WITHIN_MS;
            let attempt = self.attempt_link(
                &token,
                &target.account,
                &target.game_ids,
                target.tournament_event_id,
                just_played,
                now_ms,
            )?;
            match attempt {
                Attempt::Linked => {
                    self.links.retry_at.remove(&token);
                }
                Attempt::NoList => {
                    // Tried again at the match's next change, not before this.
                    self.links
                        .retry_at
                        .insert(token.clone(), now_ms + LINK_RETRY_MS);
                    self.links.relink_skip.insert(token);
                }
                Attempt::NoVersion => {
                    self.links.retry_at.remove(&token);
                    self.links.unmatched.insert(token.clone());
                    self.links.relink_skip.insert(token);
                    // The deck may not be synced yet (0.x nudges a sync too),
                    // unless the files were just read in this tick.
                    if !self.links.synced_this_tick {
                        self.request_deck_sync(now_ms);
                    }
                }
            }
        }

        if self.links.relink_wanted {
            self.relink_recent(now_ms)?;
        }
        self.inherit_tournament_decks()
    }

    /// 0.x `RelinkOrphanMatches::inheritTournamentDecks`, after the links
    /// above: an unlinked tournament match takes the deck version its
    /// account's other rounds of the same tournament are linked to, when they
    /// all use one. MTGO locks the registered list for the whole event, and a
    /// round's own list may be gone (round 1 often starts before the deck
    /// file is synced). No date window: historic rounds are repaired too.
    fn inherit_tournament_decks(&mut self) -> Result<(), TrackerError> {
        // The links sent above must be stored to count as siblings.
        self.flush()?;
        for sibling in tournament_sibling_versions(&self.reader, SIBLING_LIMIT)? {
            tracing::info!(
                token = %sibling.token,
                deck_version_id = sibling.deck_version_id,
                "a tournament round inherited its deck version from its other rounds"
            );
            self.links.pending.remove(&sibling.token);
            self.links.unmatched.remove(&sibling.token);
            self.links.linked.insert(sibling.token.clone());
            self.mark_match_owner(&sibling.token);
            self.batch.decks.push(DeckWrite::Link {
                match_token: sibling.token,
                deck_version_id: sibling.deck_version_id,
            });
            self.decks_changed = true;
            // A tournament card's version label and the deck's tournaments
            // follow the round's deck.
            self.leagues_changed = true;
        }
        Ok(())
    }

    /// After a deck sync: try every unlinked match started within
    /// `RELINK_WITHIN_MS` again (newest first, at most `RELINK_LIMIT`), except
    /// those that did not link since the decks last changed. Never reads the
    /// deck files or asks for a sync itself.
    fn relink_recent(&mut self, now_ms: i64) -> Result<(), TrackerError> {
        self.links.relink_wanted = false;
        self.flush()?;
        let matches = unlinked_matches_since(
            &self.reader,
            now_ms - RELINK_WITHIN_MS,
            RELINK_LIMIT,
            &self.links.relink_skip,
        )?;
        for unlinked in matches {
            // Every link sent so far is committed, so a match marked linked
            // without a stored link lost it (the store skips a link whose
            // version was pruned first): it is tried again.
            self.links.linked.remove(&unlinked.token);
            let attempt = self.attempt_link(
                &unlinked.token,
                &unlinked.account,
                &unlinked.game_ids,
                unlinked.tournament_event_id,
                false,
                now_ms,
            )?;
            if attempt == Attempt::Linked {
                self.links.unmatched.remove(&unlinked.token);
            } else {
                self.links.relink_skip.insert(unlinked.token);
            }
        }
        Ok(())
    }

    /// Find the version this match was played with and queue its link.
    /// `live`: a match just played may read the deck files again on an exact
    /// miss (once per tick).
    fn attempt_link(
        &mut self,
        token: &str,
        account: &str,
        game_ids: &[Option<u64>],
        tournament_event_id: Option<u64>,
        live: bool,
        now_ms: i64,
    ) -> Result<Attempt, TrackerError> {
        let Some(played) = self.played_list(token, game_ids, tournament_event_id)? else {
            return Ok(Attempt::NoList);
        };
        let mut versions = match self.links.versions.remove(account) {
            Some(cached) => cached,
            None => account_versions(&self.reader, account)?,
        };
        let found = match played {
            Played::BeforeSideboarding(cards) => {
                let mut found = exact_version(&cards, &versions);
                if found.is_none() && live && self.read_deck_files_again(now_ms)? {
                    versions = account_versions(&self.reader, account)?;
                    found = exact_version(&cards, &versions);
                }
                match found {
                    Some(id) => Some(id),
                    None => self.oracle_version(&cards, &versions)?,
                }
            }
            Played::LaterGame(cards) => {
                let mut found = version_by_totals(&cards, &versions, &mut self.links.totals);
                if found.is_none() && live && self.read_deck_files_again(now_ms)? {
                    versions = account_versions(&self.reader, account)?;
                    found = version_by_totals(&cards, &versions, &mut self.links.totals);
                }
                found
            }
        };
        self.links.versions.insert(account.to_string(), versions);
        let Some(version_id) = found else {
            return Ok(Attempt::NoVersion);
        };
        self.batch.decks.push(DeckWrite::Link {
            match_token: token.to_string(),
            deck_version_id: version_id,
        });
        self.links.linked.insert(token.to_string());
        self.links.settled(token);
        self.decks_changed = true;
        // A league run's deck version follows its matches' links.
        if self.league_matches.contains(token)
            || store::read::is_league_or_tournament_match(&self.reader, token).unwrap_or(true)
        {
            self.mark_account(Some(account));
        }
        Ok(Attempt::Linked)
    }

    /// Users edit a deck right before queueing and MTGO writes its file while
    /// open, so an exact miss on a live match reads the deck files again and
    /// commits what changed. At most once per tick; `false` when the files
    /// were already read this tick.
    fn read_deck_files_again(&mut self, now_ms: i64) -> Result<bool, TrackerError> {
        if self.links.synced_this_tick {
            return Ok(false);
        }
        self.sync_deck_files(now_ms)?;
        self.flush()?;
        Ok(true)
    }

    /// The list the match was played with (see the module docs for the order).
    fn played_list(
        &mut self,
        token: &str,
        game_ids: &[Option<u64>],
        tournament_event_id: Option<u64>,
    ) -> Result<Option<Played>, TrackerError> {
        // Game 1 only: a later game's list is never taken as game 1's.
        if let Some(Some(game_one)) = game_ids.first()
            && let Some(cards) = self.deck_used_list(token, *game_one)?
        {
            return Ok(Some(Played::BeforeSideboarding(cards)));
        }
        if let Some(cards) = self.registered_list(token)? {
            return Ok(Some(Played::BeforeSideboarding(cards)));
        }
        if let Some(event_id) = tournament_event_id
            && let Some(cards) = tournament_deck(&self.reader, event_id)?
        {
            return Ok(Some(Played::BeforeSideboarding(cards)));
        }
        for game_id in game_ids.iter().skip(1).flatten() {
            if let Some(cards) = self.deck_used_list(token, *game_id)? {
                return Ok(Some(Played::LaterGame(cards)));
            }
        }
        Ok(None)
    }

    /// The first parseable `deck_used` list of a game: from `log_events`, else from the
    /// match archive (compaction moves a finished match's events there).
    fn deck_used_list(
        &mut self,
        token: &str,
        game_id: u64,
    ) -> Result<Option<Vec<DeckCard>>, TrackerError> {
        let raws = deck_used_raws(&self.reader, game_id)?;
        if !raws.is_empty() {
            // The first one that parses: a damaged first line does not hide a
            // later one, as the archive path below takes the first parseable.
            for raw in raws {
                if let Some((id, cards)) = parse_deck_used(&raw)
                    && id == game_id
                {
                    return Ok(Some(cards));
                }
            }
            return Ok(None);
        }
        self.load_archive(token)?;
        Ok(self
            .links
            .archives
            .get(token)
            .and_then(|lists| lists.deck_used.get(&game_id).cloned()))
    }

    /// The match's first registered list, from `log_events` or its archive.
    fn registered_list(&mut self, token: &str) -> Result<Option<Vec<DeckCard>>, TrackerError> {
        if let Some(raw) = registered_deck_raw(&self.reader, token)? {
            return Ok(parse_registered_deck(&raw)
                .filter(|(registered_for, _)| registered_for == token)
                .map(|(_, cards)| cards));
        }
        self.load_archive(token)?;
        Ok(self
            .links
            .archives
            .get(token)
            .and_then(|lists| lists.registered.clone()))
    }

    /// Read the deck lists of the match's archive into `links.archives`,
    /// unless the cached copy is of the same archive write.
    fn load_archive(&mut self, token: &str) -> Result<(), TrackerError> {
        let Some(stamp) = compacted_at(&self.reader, token)? else {
            self.links.archives.remove(token);
            return Ok(());
        };
        if self
            .links
            .archives
            .get(token)
            .is_some_and(|lists| lists.compacted_at == stamp)
        {
            return Ok(());
        }
        let mut lists = ArchivedLists {
            compacted_at: stamp,
            deck_used: HashMap::new(),
            registered: None,
        };
        // Archived events are in log order.
        let events = match archived_events(&self.reader, token) {
            Ok(events) => events,
            // A database error is not the archive's fault: fail the tick.
            Err(StoreError::Sqlite(error)) => return Err(StoreError::Sqlite(error).into()),
            // An archive that cannot be decoded gives no lists. The empty
            // answer is cached under the archive's stamp, so it is logged
            // once and not read again until the archive is rewritten.
            Err(error) => {
                tracing::warn!(%token, %error, "a match's archive could not be read for deck linking");
                Vec::new()
            }
        };
        for event in events {
            if let Some((game_id, cards)) = parse_deck_used(&event.raw) {
                lists.deck_used.entry(game_id).or_insert(cards);
            } else if lists.registered.is_none()
                && let Some((registered_for, cards)) = parse_registered_deck(&event.raw)
                && registered_for == token
            {
                lists.registered = Some(cards);
            }
        }
        self.links.archives.insert(token.to_string(), lists);
        Ok(())
    }

    /// 0.x `FindDeckVersionByOracleIdentity`: the version whose cards have the
    /// played list's oracle identity, when every played card has an oracle id.
    /// Versions of more than one deck give no answer; within one deck the
    /// newest `modified_at` wins.
    fn oracle_version(
        &mut self,
        cards: &[DeckCard],
        versions: &[VersionRow],
    ) -> Result<Option<i64>, TrackerError> {
        if versions.is_empty() {
            return Ok(None);
        }
        let played_ids: Vec<u32> = cards.iter().map(|card| card.catalog_id).collect();
        let played_oracles = oracle_ids(&self.reader, &played_ids)?;
        let Some(wanted) = oracle_key(cards, &|id| played_oracles.get(&id).cloned()) else {
            return Ok(None);
        };

        // One lookup for every card of every version without a cached key.
        let mut uncached: Vec<(&VersionRow, Vec<DeckCard>)> = Vec::new();
        for version in versions {
            let cached = self
                .links
                .oracle_keys
                .get(&version.id)
                .is_some_and(|(signature, _)| *signature == version.signature);
            if !cached {
                uncached.push((version, decode_signature(&version.signature)));
            }
        }
        let mut catalog_ids: Vec<u32> = Vec::new();
        for (_, version_cards) in &uncached {
            catalog_ids.extend(version_cards.iter().map(|card| card.catalog_id));
        }
        catalog_ids.sort_unstable();
        catalog_ids.dedup();
        let oracles = oracle_ids(&self.reader, &catalog_ids)?;
        for (version, version_cards) in uncached {
            if let Some(key) = oracle_key(&version_cards, &|id| oracles.get(&id).cloned()) {
                self.links
                    .oracle_keys
                    .insert(version.id, (version.signature.clone(), key));
            }
        }

        let mut matching: Vec<&VersionRow> = Vec::new();
        for version in versions {
            let key = self.links.oracle_keys.get(&version.id);
            if key.is_some_and(|(signature, key)| *signature == version.signature && *key == wanted)
            {
                matching.push(version);
            }
        }
        Ok(newest_of_one_deck(&matching))
    }
}

/// The first version (in id order) whose signature is the list's.
fn exact_version(cards: &[DeckCard], versions: &[VersionRow]) -> Option<i64> {
    let wanted = decks::signature(cards);
    versions
        .iter()
        .find(|version| version.signature == wanted)
        .map(|version| version.id)
}

/// How many of each printing a list holds, main and sideboard together.
fn card_totals(cards: &[DeckCard]) -> BTreeMap<u32, u64> {
    let mut totals = BTreeMap::new();
    for card in cards {
        *totals.entry(card.catalog_id).or_insert(0) += u64::from(card.quantity);
    }
    totals
}

/// The versions holding the same total of every printing as the list. Each
/// version's totals come from `cache`, decoded only when missing or stored for
/// another signature.
fn same_totals(
    cards: &[DeckCard],
    versions: &[VersionRow],
    cache: &mut HashMap<i64, VersionTotals>,
) -> Vec<i64> {
    let wanted = card_totals(cards);
    let mut fits = Vec::new();
    for version in versions {
        let cached = cache
            .get(&version.id)
            .is_some_and(|(signature, _)| *signature == version.signature);
        if !cached {
            let totals = card_totals(&decode_signature(&version.signature));
            cache.insert(version.id, (version.signature.clone(), totals));
        }
        let fits_list = cache
            .get(&version.id)
            .is_some_and(|(_, totals)| *totals == wanted);
        if fits_list {
            fits.push(version.id);
        }
    }
    fits
}

/// The version a later game's list was played with: of the versions holding
/// its totals, the newest when they are all of one deck (as the oracle path
/// picks), none when they are of several decks.
fn version_by_totals(
    cards: &[DeckCard],
    versions: &[VersionRow],
    cache: &mut HashMap<i64, VersionTotals>,
) -> Option<i64> {
    let fits = same_totals(cards, versions, cache);
    let rows: Vec<&VersionRow> = versions
        .iter()
        .filter(|version| fits.contains(&version.id))
        .collect();
    newest_of_one_deck(&rows)
}

/// The newest of these versions (lowest id on a tie, as 0.x's stable sort
/// keeps query order), or `None` when they are of more than one deck.
fn newest_of_one_deck(versions: &[&VersionRow]) -> Option<i64> {
    let first = versions.first()?;
    if versions
        .iter()
        .any(|version| version.deck_id != first.deck_id)
    {
        return None;
    }
    let mut newest = *first;
    for version in versions {
        let newer = version.modified_at > newest.modified_at
            || (version.modified_at == newest.modified_at && version.id < newest.id);
        if newer {
            newest = version;
        }
    }
    Some(newest.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrackerConfig;
    use crate::engine::DECK_SYNC_EVERY_MS;
    use crate::engine::test_support::{config as fixture_config, open, run_until_settled};
    use chrono::NaiveDate;
    use fixture_tools::synth::Synth;
    use fixture_tools::{Fixture, TestDir, load_fixture};
    use mtgo_match::Derived;
    use std::fs::File;
    use std::time::{Duration, SystemTime};
    use store::decks::{DeckEdit, deck_by_mtgo_id, link_of, versions_of};
    use store::{open_reader, read};

    const DECK_1: &str = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
    const DECK_2: &str = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d";
    const TOKEN: &str = "c1d2e3f4-a5b6-4c7d-8e9f-0a1b2c3d4e5f";
    const ME: &str = "Qlocal";
    const OPP: &str = "Qopponent";
    const ACCOUNT: &str = "Q1/A1/acct";
    /// 2026-07-01T12:30:00Z, half an hour after the synthetic match.
    const NOW: i64 = 1_782_909_000_000;

    type Card = (u32, u32, bool);
    /// The list the synthetic deck is played with, in log order.
    const PLAYED: [Card; 3] = [(1001, 4, false), (1003, 1, false), (1002, 2, true)];

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 7, 1).expect("date")
    }

    fn config(dir: &TestDir) -> TrackerConfig {
        TrackerConfig {
            db_path: dir.join("data/mymtgo.sqlite"),
            timezone: chrono_tz::UTC,
            os_timezone: None,
            log_overrides: vec![dir.join("logs")],
            arena_log_overrides: vec![dir.join("arena/Player.log")],
            mtgo_data_folder: Some(dir.join("mtgo-data")),
            replay_day_anchor: Some(day()),
            watching: true,
            arena_enabled: true,
            endstep_enabled: false,
        }
    }

    fn json_list(cards: &[Card], catalog_key: &str) -> String {
        let items: Vec<String> = cards
            .iter()
            .map(|(catalog_id, quantity, sideboard)| {
                format!(
                    "{{\"{catalog_key}\":{catalog_id},\"Quantity\":{quantity},\"Annotation\":\"NotSet\",\"InSideboard\":{sideboard}}}"
                )
            })
            .collect();
        format!("[{}]", items.join(","))
    }

    /// What a synthetic match logs, in order.
    enum Line {
        /// A game starts (its Game ID, Match ID 99).
        Game(u64),
        DeckUsed(u64, &'static [Card]),
        Registered(&'static [Card]),
        /// The registered deck of a tournament (its id), logged at a login
        /// while it runs.
        TournamentDeck(u64, &'static [Card]),
        /// A line as it is, after the usual timestamp.
        Raw(&'static str),
        /// MTGO closes the match.
        Completed,
    }

    /// A log in which `ME` logs in and plays `TOKEN` (Match ID 99).
    fn match_log(lines: &[Line]) -> Vec<u8> {
        joined_log(&[], lines)
    }

    /// The tournament id in `tournament_round_log`'s description.
    const TOURNAMENT: u64 = 12_857_433;

    /// `match_log` for round 1 of tournament `TOURNAMENT`.
    fn tournament_round_log(lines: &[Line]) -> Vec<u8> {
        joined_log(&[("Description", "Tournament:12857433 Round:1")], lines)
    }

    fn joined_log(kv: &[(&str, &str)], lines: &[Line]) -> Vec<u8> {
        let mut s = Synth::new(Some(ME));
        let mut bodies = vec![s.joined(TOKEN, 99, 7, kv).text];
        for line in lines {
            bodies.push(match line {
                Line::Game(game_id) => s.game_state(99, *game_id, &[ME, OPP]).text,
                Line::Completed => {
                    s.state_change(TOKEN, "MatchJoinedEventUnderwayState", "MatchCompletedState")
                        .text
                }
                Line::DeckUsed(game_id, cards) => format!(
                    "12:00:00 [INF] (Twitch Info|Username: {ME} Deck Used in Game ID: {game_id}) {}",
                    json_list(cards, "CatalogId")
                ),
                Line::TournamentDeck(id, cards) => tournament_deck_line("12:00:00", *id, cards),
                Line::Raw(text) => format!("12:00:00 [INF] {text}"),
                Line::Registered(cards) => format!(
                    "12:00:00 [INF] (BaseClient|Inbound: FlsMatchDeckGetRespMessage) \
                     {{\"MatchToken\":\"{TOKEN}\",\"MatchID\":99,\"Cards\":{}}}",
                    json_list(cards, "CatalogID")
                ),
            });
        }
        let mut text = format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n");
        for (index, body) in bodies.iter().enumerate() {
            // Synthetic lines all start with "12:00:00"; give each its own second.
            let body = body.trim_end();
            text.push_str(&format!("12:00:{:02}{}\n", index + 1, &body[8..]));
        }
        text.push_str("12:00:59 [INF] (UI|after)\n");
        text.into_bytes()
    }

    /// A tournament deck line (with its `Receiver` tail) logged at `time`.
    fn tournament_deck_line(time: &str, id: u64, cards: &[Card]) -> String {
        format!(
            "{time} [INF] (Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in TournamentJoinedRoundInProgressState) \
             Processor: TournamentJoinedRoundInProgressState Message: \
             {{\"TournamentToken\":\"c8eed54f-8182-44d5-adb0-dc8cb4ac95f7\",\"TournamentID\":{id},\"DeckCards\":{},\"DeckName\":\"Qdeck\",\"DeckBoxID\":1,\"ReturnCode\":0}}\n\
             }} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament",
            json_list(cards, "CatalogID")
        )
    }

    /// The usual synthetic match: game 1 logs its list.
    fn played_log() -> Vec<u8> {
        match_log(&[Line::Game(7), Line::DeckUsed(7, &PLAYED)])
    }

    fn deck_xml(id: &str, name: &str, timestamp: &str, cards: &[Card]) -> String {
        let mut xml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Grouping Name=\"{name}\" NetDeckId=\"{id}\" \
             GroupingType=\"Deck\" Timestamp=\"{timestamp}\" FormatCode=\"CMODERN\">\r\n"
        );
        for (catalog_id, quantity, sideboard) in cards {
            let side = if *sideboard { "True" } else { "False" };
            xml.push_str(&format!(
                "  <Item CatId=\"{catalog_id}\" Quantity=\"{quantity}\" IsSideboard=\"{side}\" />\r\n"
            ));
        }
        xml.push_str("</Grouping>");
        xml
    }

    /// Write a deck file into the account folder (made the newest one).
    fn write_deck(dir: &TestDir, id: &str, timestamp: &str, cards: &[Card]) {
        let settings = dir.write(&format!("mtgo-data/{ACCOUNT}/user_settings"), b"x");
        File::options()
            .write(true)
            .open(&settings)
            .expect("open")
            .set_modified(SystemTime::now() - Duration::from_secs(60))
            .expect("set modified");
        let xml = deck_xml(id, "Qdeck", timestamp, cards);
        dir.write(
            &format!("mtgo-data/{ACCOUNT}/grouping {id}.xml"),
            xml.as_bytes(),
        );
    }

    /// Card rows with oracle ids: `(catalog id, oracle id)`.
    fn oracle_cards(config: &TrackerConfig, cards: &[(u32, &str)]) {
        let conn = store::open_writer(&config.db_path).expect("writer");
        for (catalog_id, oracle_id) in cards {
            conn.execute_batch(&format!(
                "INSERT INTO cards (catalog_id, oracle_id, name) VALUES ({catalog_id}, '{oracle_id}', 'Qcard');"
            ))
            .expect("card");
        }
    }

    /// The only version of a deck.
    fn only_version(config: &TrackerConfig, mtgo_id: &str) -> i64 {
        let reader = open_reader(&config.db_path).expect("reader");
        let deck = deck_by_mtgo_id(&reader, mtgo_id)
            .expect("deck")
            .expect("synced");
        let versions = versions_of(&reader, deck.id).expect("versions");
        assert_eq!(versions.len(), 1, "one version");
        versions[0].id
    }

    fn link(config: &TrackerConfig, token: &str) -> Option<i64> {
        let reader = open_reader(&config.db_path).expect("reader");
        link_of(&reader, token).expect("link")
    }

    fn revision(config: &TrackerConfig, token: &str) -> Option<u64> {
        let reader = open_reader(&config.db_path).expect("reader");
        read::match_revision(&reader, token).expect("revision")
    }

    /// The only match a fixture stores.
    fn only_token(config: &TrackerConfig) -> String {
        let reader = open_reader(&config.db_path).expect("reader");
        let rows = read::list_matches(&reader, 10, 0).expect("list");
        assert_eq!(rows.len(), 1, "one match");
        rows[0].token.clone()
    }

    /// The cards of the first `deck_used` line in a fixture.
    fn first_deck_used(fixture: &Fixture) -> Vec<Card> {
        let text = String::from_utf8_lossy(&fixture.bytes);
        let line = text
            .lines()
            .find(|line| line.contains("Deck Used in Game ID"))
            .expect("a deck_used line");
        let (_, cards) = decks::parse_deck_used(line).expect("parses");
        cards
            .into_iter()
            .map(|card| (card.catalog_id, card.quantity, card.sideboard))
            .collect()
    }

    fn version(id: i64, deck_id: i64, modified_at: i64) -> VersionRow {
        VersionRow {
            id,
            deck_id,
            signature: String::new(),
            modified_at,
        }
    }

    #[test]
    fn the_oracle_pick_is_the_newest_version_of_a_single_deck() {
        let (a, b, c) = (
            version(1, 10, 500),
            version(2, 10, 900),
            version(3, 10, 900),
        );
        assert_eq!(
            newest_of_one_deck(&[&a, &b, &c]),
            Some(2),
            "a tie keeps the lower id"
        );
        assert_eq!(newest_of_one_deck(&[&a]), Some(1));
        let other_deck = version(4, 11, 100);
        assert_eq!(newest_of_one_deck(&[&a, &other_deck]), None);
        assert_eq!(newest_of_one_deck(&[]), None);
    }

    #[test]
    fn totals_ignore_main_and_sideboard_and_merge_rows() {
        let card = |catalog_id, quantity, sideboard| DeckCard {
            catalog_id,
            quantity,
            sideboard,
        };
        let deck = [card(1, 4, false), card(2, 2, true)];
        let boarded = [card(2, 1, false), card(1, 4, false), card(2, 1, true)];
        assert_eq!(card_totals(&deck), card_totals(&boarded));
        let swapped = [card(1, 3, false), card(2, 3, true)];
        assert_ne!(card_totals(&deck), card_totals(&swapped));
        let versions = [
            VersionRow {
                signature: decks::signature(&deck),
                ..version(7, 1, 0)
            },
            VersionRow {
                signature: decks::signature(&swapped),
                ..version(8, 2, 0)
            },
        ];
        assert_eq!(
            same_totals(&boarded, &versions, &mut HashMap::new()),
            vec![7]
        );
    }

    #[test]
    fn a_versions_totals_are_decoded_once_per_signature() {
        let card = |catalog_id, quantity, sideboard| DeckCard {
            catalog_id,
            quantity,
            sideboard,
        };
        let deck = [card(1, 4, false), card(2, 2, true)];
        let versions = [VersionRow {
            signature: decks::signature(&deck),
            ..version(7, 1, 0)
        }];
        let mut cache = HashMap::new();
        assert_eq!(same_totals(&deck, &versions, &mut cache), vec![7]);

        // The cached totals are used as they are: no decode.
        let (_, totals) = cache.get_mut(&7).expect("cached");
        totals.clear();
        assert!(same_totals(&deck, &versions, &mut cache).is_empty());

        // A version id with another signature is decoded again.
        let other = [card(3, 4, false)];
        let replaced = [VersionRow {
            signature: decks::signature(&other),
            ..version(7, 1, 0)
        }];
        assert_eq!(same_totals(&other, &replaced, &mut cache), vec![7]);
    }

    /// MTGO logs the registered list before sideboarding: the first one of a
    /// match is exactly the game 1 list logged before it, and every one keeps
    /// the totals of the list before it.
    #[test]
    fn fixture_registered_lists_repeat_the_list_played_before_them() {
        for (name, count) in [("draft_hobbit", 5), ("league_join_drop", 1)] {
            let fixture = load_fixture(name);
            let text = String::from_utf8_lossy(&fixture.bytes).to_string();
            let mut last_played: Option<Vec<DeckCard>> = None;
            let mut seen: HashSet<String> = HashSet::new();
            let mut registered = 0;
            for line in text.lines() {
                if let Some((_, cards)) = parse_deck_used(line) {
                    last_played = Some(cards);
                    continue;
                }
                if !line.contains("FlsMatchDeckGetRespMessage") {
                    continue;
                }
                let (token, cards) =
                    parse_registered_deck(line).unwrap_or_else(|| panic!("{name}: parses"));
                registered += 1;
                let played = last_played.as_ref().expect("a list was played before");
                assert_eq!(card_totals(&cards), card_totals(played), "{name}");
                if seen.insert(token) {
                    assert_eq!(
                        decks::signature(&cards),
                        decks::signature(played),
                        "{name}: the first registered list is game 1's"
                    );
                }
            }
            assert_eq!(registered, count, "{name}");
        }
    }

    #[test]
    fn a_fixture_match_links_the_deck_file_with_game_ones_list() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("link-fixture");
        dir.write("logs/mtgo.log", &fixture.bytes);
        write_deck(
            &dir,
            DECK_1,
            "2026-08-20T10:00:00",
            &first_deck_used(&fixture),
        );
        let config = fixture_config(&dir, &fixture);
        let mut engine = open(&config, fixture.meta.now_ms());
        run_until_settled(&mut engine, fixture.meta.now_ms());

        let token = only_token(&config);
        assert_eq!(link(&config, &token), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_fixture_match_without_game_ones_list_links_by_its_registered_list() {
        let fixture = load_fixture("league_join_drop");
        let played = first_deck_used(&fixture);
        // Drop game 1's `deck_used` line: the registered list after game 1 remains.
        let text = String::from_utf8_lossy(&fixture.bytes).to_string();
        let first = text
            .find("Deck Used in Game ID: 945500844")
            .expect("game 1");
        let start = text[..first].rfind('\n').expect("line start") + 1;
        let end = start + text[start..].find('\n').expect("line end") + 1;
        let without = format!("{}{}", &text[..start], &text[end..]);
        assert!(!without.contains("Deck Used in Game ID: 945500844"));

        let dir = TestDir::new("link-registered");
        dir.write("logs/mtgo.log", without.as_bytes());
        write_deck(&dir, DECK_1, "2026-08-20T10:00:00", &played);
        let config = fixture_config(&dir, &fixture);
        let mut engine = open(&config, fixture.meta.now_ms());
        run_until_settled(&mut engine, fixture.meta.now_ms());

        let token = only_token(&config);
        assert_eq!(link(&config, &token), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_deck_file_listing_the_cards_in_another_order_links() {
        let dir = TestDir::new("link-reordered");
        dir.write("logs/mtgo.log", &played_log());
        let mut reordered = PLAYED.to_vec();
        reordered.reverse();
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &reordered);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_different_printing_links_through_oracle_ids() {
        let dir = TestDir::new("link-oracle");
        dir.write("logs/mtgo.log", &played_log());
        // 2001 is another printing of 1001.
        write_deck(
            &dir,
            DECK_1,
            "2026-06-30T10:00:00",
            &[(2001, 4, false), (1003, 1, false), (1002, 2, true)],
        );
        let config = config(&dir);
        drop(store::open_writer(&config.db_path).expect("migrate"));
        oracle_cards(
            &config,
            &[
                (1001, "o-bolt"),
                (2001, "o-bolt"),
                (1002, "o-land"),
                (1003, "o-relic"),
            ],
        );
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn imported_card_data_relinks_at_the_next_tick() {
        let dir = TestDir::new("link-after-import");
        dir.write("logs/mtgo.log", &played_log());
        // 2001 is another printing of 1001: only oracle ids can link it.
        write_deck(
            &dir,
            DECK_1,
            "2026-06-30T10:00:00",
            &[(2001, 4, false), (1003, 1, false), (1002, 2, true)],
        );
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert_eq!(link(&config, TOKEN), None, "no card data yet");

        let card = |catalog_id: i64, oracle_id: &str| store::import_decks::ImportedCard {
            catalog_id,
            scryfall_id: None,
            oracle_id: Some(oracle_id.to_string()),
            name: Some("Qcard".to_string()),
            card_type: None,
            sub_type: None,
            rarity: None,
            color_identity: None,
            colors: None,
            cmc: None,
            mana_cost: None,
            set_name: None,
            set_code: None,
            image: None,
            art_crop: None,
        };
        let data = store::import_decks::ImportedDecks {
            cards: vec![
                card(1001, "o-bolt"),
                card(2001, "o-bolt"),
                card(1002, "o-land"),
                card(1003, "o-relic"),
            ],
            ..store::import_decks::ImportedDecks::default()
        };
        engine.import_decks(data, now).expect("import");
        let at = now + 1_000;
        assert!(!engine.deck_sync_due(at), "no scheduled or requested sync");
        engine.tick(at).expect("tick");
        engine.flush().expect("flush");

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn two_decks_with_the_played_oracle_identity_leave_the_match_unlinked() {
        let dir = TestDir::new("link-oracle-ambiguous");
        dir.write("logs/mtgo.log", &played_log());
        let other_printing = [(2001, 4, false), (1003, 1, false), (1002, 2, true)];
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &other_printing);
        write_deck(&dir, DECK_2, "2026-06-30T11:00:00", &other_printing);
        let config = config(&dir);
        drop(store::open_writer(&config.db_path).expect("migrate"));
        oracle_cards(
            &config,
            &[
                (1001, "o-bolt"),
                (2001, "o-bolt"),
                (1002, "o-land"),
                (1003, "o-relic"),
            ],
        );
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        only_version(&config, DECK_1);
        only_version(&config, DECK_2);
        assert_eq!(link(&config, TOKEN), None);
    }

    #[test]
    fn an_archived_decks_version_still_links() {
        let dir = TestDir::new("link-archived");
        dir.write(
            "logs/mtgo.log",
            format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n12:00:00 [INF] (UI|x)\n")
                .as_bytes(),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);
        let reader = open_reader(&config.db_path).expect("reader");
        let deck = deck_by_mtgo_id(&reader, DECK_1)
            .expect("deck")
            .expect("synced");
        engine
            .writer
            .edit_deck(DeckEdit::Archive {
                deck_id: deck.id,
                at_ms: NOW,
            })
            .expect("archive");

        dir.append("logs/mtgo.log", &played_log());
        run_until_settled(&mut engine, NOW + 10_000);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_later_deck_sync_relinks_without_changing_the_match() {
        let dir = TestDir::new("link-relink");
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert_eq!(link(&config, TOKEN), None, "no deck file yet");
        let before = revision(&config, TOKEN);
        assert!(before.is_some());

        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        engine.tick(now + DECK_SYNC_EVERY_MS).expect("tick");
        engine.flush().expect("flush");

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
        assert_eq!(
            revision(&config, TOKEN),
            before,
            "the match row is untouched"
        );
    }

    #[test]
    fn a_live_miss_after_reading_the_files_again_asks_for_no_second_sync() {
        let dir = TestDir::new("link-miss-no-request");
        dir.write(
            "logs/mtgo.log",
            format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n12:00:00 [INF] (UI|x)\n")
                .as_bytes(),
        );
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);

        // No deck fits: the files are read again in this tick, so asking for
        // another read at the next tick would find nothing new.
        dir.append("logs/mtgo.log", &played_log());
        let at = now + 1_000;
        assert!(!engine.deck_sync_due(at));
        engine.tick(at).expect("tick");
        assert_eq!(link(&config, TOKEN), None);
        assert!(!engine.deck_sync_due(at + 1), "no requested sync");
        assert!(
            engine.links.relink_skip.contains(TOKEN),
            "the relink after the read skips the match just tried"
        );
    }

    #[test]
    fn a_match_that_did_not_link_is_skipped_until_the_decks_change() {
        let dir = TestDir::new("link-skip-until-change");
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert_eq!(link(&config, TOKEN), None, "no deck file yet");
        assert!(engine.links.relink_skip.contains(TOKEN));

        // A sync that writes nothing keeps the skip.
        engine.sync_deck_files(now).expect("sync");
        engine.tick(now + 1).expect("tick");
        assert!(engine.links.relink_skip.contains(TOKEN));

        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        engine.sync_deck_files(now + 2).expect("sync");
        engine.tick(now + 3).expect("tick");
        engine.flush().expect("flush");
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_link_lost_with_its_removed_match_is_made_again_when_it_returns() {
        let dir = TestDir::new("link-removed-returns");
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert!(link(&config, TOKEN).is_some());
        let derived = engine.projector.derived(TOKEN).cloned();
        assert!(matches!(derived, Some(Derived::Visible(_))));

        // The match stops being visible: its row (no input) goes, link too.
        engine.record_derived(TOKEN.to_string(), None, 2, None);
        engine.flush().expect("flush");
        assert_eq!(link(&config, TOKEN), None);

        // It derives again: it is linked again.
        engine.record_derived(TOKEN.to_string(), derived, 3, None);
        run_until_settled(&mut engine, now + 1);
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_link_the_store_dropped_is_retried_by_the_next_relink() {
        let dir = TestDir::new("link-dropped");
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert!(link(&config, TOKEN).is_some());

        // As if the write was skipped because its version was gone.
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute("UPDATE matches SET deck_version_id = NULL", [])
            .expect("drop");
        drop(conn);
        engine.sync_deck_files(now).expect("sync");
        engine.tick(now + 1).expect("tick");
        engine.flush().expect("flush");

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_deck_file_written_after_the_last_sync_links_on_the_same_derive() {
        let dir = TestDir::new("link-reread");
        dir.write(
            "logs/mtgo.log",
            format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n12:00:00 [INF] (UI|x)\n")
                .as_bytes(),
        );
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);

        // The user saves the deck and queues: MTGO writes the file just before
        // the match, after the tracker's last sync.
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        dir.append("logs/mtgo.log", &played_log());
        let at = now + 1_000;
        assert!(!engine.deck_sync_due(at), "no scheduled or requested sync");
        engine.tick(at).expect("one tick");
        engine.flush().expect("flush");

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn an_older_match_waits_for_the_requested_sync_instead_of_reading_the_files_at_once() {
        let dir = TestDir::new("link-no-reread");
        dir.write(
            "logs/mtgo.log",
            format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n12:00:00 [INF] (UI|x)\n")
                .as_bytes(),
        );
        // The log is from the day before: its match was played a day ago.
        let config = TrackerConfig {
            replay_day_anchor: day().pred_opt(),
            ..config(&dir)
        };
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);

        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        dir.append(
            "logs/mtgo.log",
            &match_log(&[Line::Game(7), Line::DeckUsed(7, &PLAYED), Line::Completed]),
        );
        let at = now + 1_000;
        assert!(!engine.deck_sync_due(at));
        engine.tick(at).expect("tick");
        engine.flush().expect("flush");
        assert_eq!(link(&config, TOKEN), None, "no fresh read for an old match");

        assert!(engine.deck_sync_due(at + 1), "the miss asked for a sync");
        engine.tick(at + 1).expect("tick");
        engine.flush().expect("flush");
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    /// The deck as edited: 1002 starts in the sideboard.
    const BOARDED_FROM: [Card; 3] = [(1001, 4, false), (1003, 1, false), (1002, 2, true)];
    /// Game 2 after sideboarding: one 1002 came in, 1003 went out.
    const GAME_2: [Card; 4] = [
        (1001, 4, false),
        (1002, 1, false),
        (1002, 1, true),
        (1003, 1, true),
    ];

    #[test]
    fn a_later_games_list_links_the_one_version_with_the_same_card_totals() {
        let dir = TestDir::new("link-later-game");
        dir.write(
            "logs/mtgo.log",
            &match_log(&[Line::Game(7), Line::Game(8), Line::DeckUsed(8, &GAME_2)]),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &BOARDED_FROM);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_damaged_first_deck_used_line_falls_back_to_a_later_one() {
        let dir = TestDir::new("link-deck-used-fallback");
        dir.write(
            "logs/mtgo.log",
            &match_log(&[
                Line::Game(7),
                Line::Raw(
                    "(Twitch Info|Username: Qlocal Deck Used in Game ID: 7) [{\"CatalogId\":10",
                ),
                Line::DeckUsed(7, &PLAYED),
            ]),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn totals_pick_the_newest_of_several_versions_of_one_deck() {
        let card = |catalog_id, quantity, sideboard| DeckCard {
            catalog_id,
            quantity,
            sideboard,
        };
        let same = [card(1, 4, false), card(2, 2, true)];
        let version_of = |id, deck_id, modified_at, cards: &[DeckCard]| VersionRow {
            signature: decks::signature(cards),
            ..version(id, deck_id, modified_at)
        };
        let versions = [
            version_of(1, 10, 100, &same),
            version_of(2, 10, 900, &same),
            version_of(3, 11, 500, &[card(5, 1, false)]),
        ];
        let mut cache = HashMap::new();
        let other = [card(9, 1, false)];
        assert_eq!(version_by_totals(&other, &versions, &mut cache), None);
        let boarded = [card(1, 3, false), card(1, 1, true), card(2, 2, true)];
        assert_eq!(
            version_by_totals(&boarded, &versions, &mut cache),
            Some(2),
            "the newest of one deck"
        );
        // Another deck with the same totals makes it ambiguous again.
        let mut two_decks = versions.to_vec();
        two_decks.push(version_of(4, 12, 50, &same));
        assert_eq!(version_by_totals(&boarded, &two_decks, &mut cache), None);
    }

    #[test]
    fn a_match_older_than_the_relink_window_is_not_linked_by_a_rebuild() {
        let dir = TestDir::new("link-old-match");
        dir.write("logs/mtgo.log", &played_log());
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let old_config = config(&dir);
        let later = NOW + RELINK_WITHIN_MS + 24 * 60 * 60 * 1000;
        let mut engine = open(&old_config, later);
        run_until_settled(&mut engine, later);
        assert_eq!(link(&old_config, TOKEN), None);

        // The same match inside the window links.
        let dir = TestDir::new("link-recent-match");
        dir.write("logs/mtgo.log", &played_log());
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);
        assert!(link(&config, TOKEN).is_some());
    }

    #[test]
    fn link_state_leaves_with_the_relink_window_through_the_engine() {
        let dir = TestDir::new("link-prune-window");
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert!(engine.links.unmatched.contains(TOKEN), "no deck: unmatched");
        assert!(engine.links.started.contains_key(TOKEN));

        let later = now + RELINK_WITHIN_MS + 1_000;
        engine.tick(later).expect("tick");
        assert!(engine.links.unmatched.is_empty());
        assert!(engine.links.relink_skip.is_empty());
        assert!(engine.links.retry_at.is_empty());
        assert!(engine.links.archives.is_empty());
        assert!(engine.links.started.is_empty());
    }

    #[test]
    fn a_match_that_links_drops_its_failure_marks_through_the_engine() {
        let dir = TestDir::new("link-prune-linked");
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert!(engine.links.unmatched.contains(TOKEN));
        assert!(engine.links.relink_skip.contains(TOKEN));

        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        engine.tick(now + DECK_SYNC_EVERY_MS).expect("tick");
        engine.flush().expect("flush");
        assert!(link(&config, TOKEN).is_some());
        assert!(!engine.links.unmatched.contains(TOKEN));
        assert!(!engine.links.relink_skip.contains(TOKEN));
        assert!(!engine.links.retry_at.contains_key(TOKEN));
    }

    #[test]
    fn forgetting_a_match_prunes_all_its_link_state() {
        let mut state = LinkState::default();
        let token = "t";
        state.unmatched.insert(token.to_string());
        state.retry_at.insert(token.to_string(), 5);
        state.relink_skip.insert(token.to_string());
        state.linked.insert(token.to_string());
        state.archives.insert(
            token.to_string(),
            ArchivedLists {
                compacted_at: 1,
                deck_used: HashMap::new(),
                registered: None,
            },
        );
        state.forget(token);
        assert!(state.unmatched.is_empty());
        assert!(state.retry_at.is_empty());
        assert!(state.relink_skip.is_empty());
        assert!(state.linked.is_empty());
        assert!(state.archives.is_empty());
    }

    #[test]
    fn a_later_games_list_matching_two_versions_by_totals_links_nothing() {
        let dir = TestDir::new("link-later-game-ambiguous");
        dir.write(
            "logs/mtgo.log",
            &match_log(&[Line::Game(7), Line::Game(8), Line::DeckUsed(8, &GAME_2)]),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &BOARDED_FROM);
        // Same totals, another split: also exactly the game 2 list, which must
        // never count as the pre-sideboard list.
        write_deck(&dir, DECK_2, "2026-06-30T11:00:00", &GAME_2);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        only_version(&config, DECK_2);
        assert_eq!(link(&config, TOKEN), None);
    }

    #[test]
    fn without_game_ones_list_the_registered_list_comes_before_a_later_games_list() {
        let dir = TestDir::new("link-registered-first");
        dir.write(
            "logs/mtgo.log",
            &match_log(&[
                Line::Game(7),
                Line::Registered(&BOARDED_FROM),
                Line::Game(8),
                Line::DeckUsed(8, &GAME_2),
            ]),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &BOARDED_FROM);
        write_deck(&dir, DECK_2, "2026-06-30T11:00:00", &GAME_2);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    /// Game 1 of a round MTGO logged in to after the round started.
    const LATE_GAME_ONE: Line =
        Line::Raw("(Twitch Info|Username: Qlocal Deck Used in Game ID: 7) [NULL]");

    /// Round 1 of `TOURNAMENT` after a late login: game 1 logs no list.
    fn late_login_round(name: &str, deck_line: Line) -> (TestDir, TrackerConfig) {
        let dir = TestDir::new(name);
        dir.write(
            "logs/mtgo.log",
            &tournament_round_log(&[deck_line, Line::Game(7), LATE_GAME_ONE]),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        run_until_settled(&mut engine, NOW);
        (dir, config)
    }

    #[test]
    fn a_round_whose_game_one_logs_no_list_links_by_the_tournaments_list() {
        let (_dir, config) = late_login_round(
            "link-tournament-deck",
            Line::TournamentDeck(TOURNAMENT, &PLAYED),
        );
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn a_tournament_list_logged_after_the_first_attempt_links_at_the_next_change() {
        let dir = TestDir::new("link-tournament-deck-later");
        dir.write(
            "logs/mtgo.log",
            &tournament_round_log(&[Line::Game(7), LATE_GAME_ONE]),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert_eq!(link(&config, TOKEN), None, "no list yet");

        let mut s = Synth::new(Some(ME));
        let game_state = s.game_state(99, 7, &[ME, OPP]).text;
        let more = format!(
            "{}\n12:01:01{}\n12:01:02 [INF] (UI|after)\n",
            tournament_deck_line("12:01:00", TOURNAMENT, &PLAYED),
            &game_state.trim_end()[8..]
        );
        dir.append("logs/mtgo.log", more.as_bytes());
        run_until_settled(&mut engine, now + LINK_RETRY_MS);

        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn another_tournaments_list_does_not_link_a_round() {
        let (_dir, config) = late_login_round(
            "link-tournament-deck-other",
            Line::TournamentDeck(TOURNAMENT + 1, &PLAYED),
        );
        assert_eq!(link(&config, TOKEN), None);
    }

    #[test]
    fn a_finished_tournaments_empty_list_does_not_link_a_round() {
        let (_dir, config) = late_login_round(
            "link-tournament-deck-empty",
            Line::TournamentDeck(TOURNAMENT, &[]),
        );
        assert_eq!(link(&config, TOKEN), None);
    }

    #[test]
    fn a_compacted_match_relinks_from_its_archive() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("link-archive");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = fixture_config(&dir, &fixture);
        let mut engine = open(&config, fixture.meta.now_ms());
        let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
        let now = now + crate::engine::COMPACT_EVERY_MS + 1;
        engine.tick(now).expect("compaction tick");
        engine.flush().expect("flush");
        let token = only_token(&config);
        let reader = open_reader(&config.db_path).expect("reader");
        let live: i64 = reader
            .query_row(
                "SELECT COUNT(*) FROM log_events WHERE kind = 'deck_used'",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(live, 0, "deck_used events are archived");
        assert_eq!(link(&config, &token), None);

        write_deck(
            &dir,
            DECK_1,
            "2026-08-20T10:00:00",
            &first_deck_used(&fixture),
        );
        engine.sync_deck_files(now + 1).expect("sync");
        engine.tick(now + 2).expect("tick");
        engine.flush().expect("flush");

        assert_eq!(link(&config, &token), Some(only_version(&config, DECK_1)));
    }

    #[test]
    fn an_unreadable_archive_gives_no_lists_and_does_not_fail_the_tick() {
        let fixture = load_fixture("league_join_drop");
        let dir = TestDir::new("link-archive-corrupt");
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = fixture_config(&dir, &fixture);
        let mut engine = open(&config, fixture.meta.now_ms());
        let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
        let now = now + crate::engine::COMPACT_EVERY_MS + 1;
        engine.tick(now).expect("compaction tick");
        engine.flush().expect("flush");
        let token = only_token(&config);
        store::open_writer(&config.db_path)
            .expect("writer")
            .execute(
                "UPDATE match_event_archives SET raw = x'00', compacted_at = compacted_at + 1",
                [],
            )
            .expect("corrupt the archive");

        write_deck(
            &dir,
            DECK_1,
            "2026-08-20T10:00:00",
            &first_deck_used(&fixture),
        );
        engine.sync_deck_files(now + 1).expect("sync");
        engine.tick(now + 2).expect("the tick survives the archive");
        engine.tick(now + 3).expect("and the next one");
        engine.flush().expect("flush");

        assert_eq!(link(&config, &token), None);
    }

    #[test]
    fn a_new_link_reports_decks_changed_once() {
        let dir = TestDir::new("link-event");
        dir.write(
            "logs/mtgo.log",
            format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n12:00:00 [INF] (UI|x)\n")
                .as_bytes(),
        );
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, mut now) = run_until_settled(&mut engine, NOW);
        assert!(
            only_version(&config, DECK_1) > 0,
            "the deck is synced first"
        );

        dir.append("logs/mtgo.log", &played_log());
        let mut reported = 0;
        for _ in 0..5 {
            now += 1_000;
            let tick = engine.tick(now).expect("tick");
            reported += tick
                .events
                .iter()
                .filter(|event| matches!(event, crate::TrackerEvent::DecksChanged))
                .count();
        }
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
        assert_eq!(reported, 1);
    }

    #[test]
    fn a_deck_set_by_hand_survives_a_relink_a_rederive_and_a_reopen() {
        let dir = TestDir::new("link-set-by-hand");
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        write_deck(&dir, DECK_2, "2026-06-30T11:00:00", &[(1001, 4, false)]);
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));
        let chosen = only_version(&config, DECK_2);

        // Only a match the edit changed has its league runs synced again: a
        // missing token marks no owner (it would mark every one).
        engine.leagues.every_owner_dirty = false;
        engine.leagues.dirty_owners.clear();
        let outcome = engine
            .edit_match(store::match_edits::MatchEdit::SetDeckVersion {
                tokens: vec!["Q-missing".to_string(), TOKEN.to_string()],
                deck_version_id: chosen,
            })
            .expect("edit");
        assert_eq!(outcome, store::match_edits::MatchEditOutcome::Done);
        assert!(!engine.leagues.every_owner_dirty);
        assert_eq!(
            engine.leagues.dirty_owners.len(),
            1,
            "the changed match's owner"
        );
        engine.leagues.dirty_owners.clear();
        let again = engine
            .edit_match(store::match_edits::MatchEdit::SetDeckVersion {
                tokens: vec![TOKEN.to_string()],
                deck_version_id: chosen,
            })
            .expect("edit");
        assert_eq!(again, store::match_edits::MatchEditOutcome::Done);
        assert!(
            engine.leagues.dirty_owners.is_empty() && !engine.leagues.every_owner_dirty,
            "an unchanged link marks nothing"
        );
        // The league sync waits for its deadline, and holds its event until then.
        let mut events = Vec::new();
        for at in 1..=3 {
            events.extend(engine.tick(now + at * 10_000).expect("tick").events);
        }
        assert!(
            events
                .iter()
                .any(|event| matches!(event, crate::TrackerEvent::DecksChanged)),
            "the deck pages refetch: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, crate::TrackerEvent::LeaguesChanged)),
            "a run's deck follows its matches: {events:?}"
        );
        assert_eq!(link(&config, TOKEN), Some(chosen));

        // A deck sync and its relink pass, then the match derived again.
        let now = now + 40_000;
        engine.sync_deck_files(now).expect("sync");
        engine.tick(now + 1).expect("tick");
        let derived = engine.projector.derived(TOKEN).cloned();
        engine.record_derived(TOKEN.to_string(), derived, 3, None);
        run_until_settled(&mut engine, now + 2);
        assert_eq!(link(&config, TOKEN), Some(chosen), "live equals rebuild");

        // A new engine over the same storage catches up and keeps it.
        drop(engine);
        let mut engine = open(&config, now + 10_000);
        run_until_settled(&mut engine, now + 10_000);
        assert_eq!(link(&config, TOKEN), Some(chosen));
    }

    #[test]
    fn removing_a_linked_match_reports_decks_changed() {
        let dir = TestDir::new("link-removed");
        write_deck(&dir, DECK_1, "2026-06-30T10:00:00", &PLAYED);
        dir.write("logs/mtgo.log", &played_log());
        let config = config(&dir);
        let mut engine = open(&config, NOW);
        let (_, now) = run_until_settled(&mut engine, NOW);
        assert_eq!(link(&config, TOKEN), Some(only_version(&config, DECK_1)));

        let revision = revision(&config, TOKEN).expect("stored") + 1;
        engine.record_derived(TOKEN.to_string(), Some(Derived::Hidden), revision, None);
        let tick = engine.tick(now + 1_000).expect("tick");

        let removed = tick
            .events
            .iter()
            .any(|event| matches!(event, crate::TrackerEvent::MatchRemoved { .. }));
        let reported = tick
            .events
            .iter()
            .filter(|event| matches!(event, crate::TrackerEvent::DecksChanged))
            .count();
        assert!(removed, "events: {:?}", tick.events);
        assert_eq!(reported, 1, "the deck lost a match: its numbers changed");
    }

    // Tournament siblings

    /// Qlocal's unlinked round `Q-round-0` of tournament 500 and one linked
    /// round per version in `sibling_versions`; Qother's round of the same
    /// tournament uses version 11.
    fn tournament_rounds(dir: &TestDir, sibling_versions: &[i64]) -> TrackerConfig {
        dir.write("logs/mtgo.log", b"12:00:00 [INF] (UI|x)\n");
        let config = config(dir);
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0), (2, 'Qother', 0);
             INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES (1, 'Q-deck', 1, 'Qdeck', 'CMODERN', 1, 1, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (10, 1, 'Q-one', 1), (11, 1, 'Q-two', 2);",
        )
        .expect("decks");
        let mut rounds: Vec<(String, i64, Option<i64>)> = vec![("Q-round-0".to_string(), 1, None)];
        for (index, version) in sibling_versions.iter().enumerate() {
            rounds.push((format!("Q-round-{}", index + 1), 1, Some(*version)));
        }
        rounds.push(("Q-other-round".to_string(), 2, Some(11)));
        for (minutes, (token, account, version)) in rounds.iter().enumerate() {
            let round = minutes as i64 + 1;
            let started = NOW - 60 * 60_000 + minutes as i64 * 60_000;
            conn.execute_batch(&format!(
                "INSERT INTO matches (token, account_id, source, tournament_event_id,
                     tournament_round, state, outcome, started_at, last_activity_at, revision,
                     derived_version)
                 VALUES ('{token}', {account}, 'log', 500, {round}, 'complete', 'win', {started},
                     {started}, 1, 4);"
            ))
            .expect("round");
            if let Some(version) = version {
                conn.execute_batch(&format!(
                    "UPDATE matches SET deck_version_id = {version} WHERE token = '{token}';"
                ))
                .expect("link");
            }
        }
        config
    }

    /// The relink pass after a deck sync.
    fn relink(config: &TrackerConfig) {
        let mut engine = open(config, NOW);
        engine.links.synced();
        engine.link_matches(NOW).expect("link");
        engine.flush().expect("flush");
    }

    #[test]
    fn an_unlinked_tournament_round_adopts_the_one_version_its_rounds_use() {
        let dir = TestDir::new("link-tournament-sibling");
        let config = tournament_rounds(&dir, &[10, 10]);
        relink(&config);
        assert_eq!(
            link(&config, "Q-round-0"),
            Some(10),
            "another account's round of the same tournament does not count"
        );
    }

    #[test]
    fn a_round_adopting_its_deck_reports_leagues_changed() {
        let dir = TestDir::new("link-tournament-sibling-event");
        let config = tournament_rounds(&dir, &[10]);
        let mut engine = open(&config, NOW);
        engine.links.synced();
        engine.link_matches(NOW).expect("link");
        let tick = engine.tick(NOW + 1_000).expect("tick");
        assert_eq!(link(&config, "Q-round-0"), Some(10));
        let reported = tick
            .events
            .iter()
            .filter(|event| matches!(event, crate::TrackerEvent::LeaguesChanged))
            .count();
        assert_eq!(reported, 1, "events: {:?}", tick.events);
    }

    #[test]
    fn rounds_using_two_versions_leave_an_unlinked_round_alone() {
        let dir = TestDir::new("link-tournament-siblings-disagree");
        let config = tournament_rounds(&dir, &[10, 11]);
        relink(&config);
        assert_eq!(link(&config, "Q-round-0"), None);
    }
}
