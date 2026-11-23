//! Matches imported from the 0.x database (importer spec sections 4 and 5).
//! Imported rows have `source = 'imported'`; a live (`source = 'log'`) row always
//! wins, whether it shares the token or the MTGO match ID, and a manual row
//! (`source = 'manual'`) is never replaced. Only the 0.x note of a
//! skipped match is kept, on the live row. A history import (Plan M,
//! `source = 'history'`) of the same match, by token or MTGO match ID, gives
//! way to the 0.x copy.
//!
//! A match the player entered by hand in 0.x (`ImportedMatch::manual`) comes
//! in as a manual match (Plan L): `source = 'manual'`, no MTGO match id, and
//! each game's hand, sideboarding and reveals as its game rows. Once
//! stored it is the player's in v1, so a rerun never rewrites it: it only
//! fills a game v1 has no record for yet (a hand, sideboarding or reveals
//! recorded in 0.x since). A row an earlier import stored as plain
//! `imported` becomes manual.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;
use crate::manual::ManualGameDetail;
use crate::write::registry_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedGame {
    pub mtgo_game_id: Option<i64>,
    pub game_number: u32,
    /// "win" | "loss" | "draw" | "unknown"
    pub result: String,
    pub on_play: Option<bool>,
    pub local_mulligans: Option<i64>,
    pub opp_mulligans: Option<i64>,
    pub local_die_roll: Option<i64>,
    pub opp_die_roll: Option<i64>,
    pub turn_count: Option<i64>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedMatch {
    pub token: String,
    pub mtgo_match_id: Option<i64>,
    pub local_username: String,
    pub opponent: Option<String>,
    pub format: Option<String>,
    pub match_type: Option<String>,
    pub tournament_event_id: Option<i64>,
    pub tournament_round: Option<i64>,
    /// "complete" | "ended" | "abandoned"
    pub state: String,
    /// "win" | "loss" | "draw" | "unknown"
    pub outcome: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub note: Option<String>,
    pub games: Vec<ImportedGame>,
    /// `Some` for a match the player entered by hand in 0.x
    /// (`matches.manual`): it is stored as a manual match
    /// (`source = 'manual'`), with each listed game's hand, sideboarding and
    /// revealed cards. `None` for a match 0.x tracked from the log.
    pub manual: Option<Vec<ManualGameDetail>>,
    /// 0.x's game score (`matches.games_won`, `games_lost`) of a match with
    /// no game rows, kept in `games_won_imported` and `games_lost_imported`
    /// so the deck stats fold it in as 0.x did (Plan O). `None` for a match
    /// with games: their rows are the score.
    pub games_won_imported: Option<i64>,
    pub games_lost_imported: Option<i64>,
    /// 0.x links the match to a deck version the deck import brings over.
    /// That link wins over a history import's (Plan M ruling I1, issue
    /// #180): taking over a history copy drops the copy's link, and the deck
    /// import then writes 0.x's. Not stored: `false` once read back.
    pub deck_linked: bool,
    /// 0.x's shared replay link of the match (`matches.replay_share_*`,
    /// replays spec section 10): stored where v1 has none. `None` for a
    /// match 0.x never shared, and always for a pulled match (bundles never
    /// carry it).
    pub replay_share: Option<ImportedReplayShare>,
    /// 0.x's `matches.imported` (issue #313): true for a match 0.x imported
    /// from MTGO's match history (its deck lists hold only the cards seen),
    /// false for one 0.x tracked from the log. A 0.x without the column (it
    /// came with 0.x's history import) or a bundle without the key is false.
    /// Stored in `matches.legacy_history`.
    pub legacy_history: bool,
}

/// A shared replay link: all three parts, or no link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedReplayShare {
    pub uuid: String,
    pub url: String,
    /// UTC unix ms.
    pub shared_at_ms: i64,
}

/// One 0.x account (Plan O): every account 0.x has not deleted, with or
/// without matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedAccount {
    pub username: String,
    /// The MTGO login id (`accounts.login_id`).
    pub login_id: Option<i64>,
    /// When 0.x registered it (`accounts.created_at`), UTC unix ms.
    pub created_at_ms: Option<i64>,
    /// 0.x records its matches (`accounts.tracked`; an older 0.x without the
    /// column tracked every account). An untracked one arrives untracked
    /// unless the user set tracking in v1 (issue #310).
    pub tracked: bool,
    /// `Some` for 0.x's active account (`accounts.active`): when 0.x last
    /// made it active, as near as 0.x tells (`updated_at`, else
    /// `created_at`, else 0), UTC unix ms. It seeds v1's switch
    /// (`active_account`) when v1 has none.
    pub active_since_ms: Option<i64>,
}

/// What an account import did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AccountImportCounts {
    /// Registered in v1 now.
    pub added: u32,
    /// Already in v1.
    pub unchanged: u32,
    /// A v1 account without a login id took 0.x's.
    pub login_ids_filled: u32,
    /// 0.x's login id belongs to another v1 account: v1's is kept.
    pub login_id_clashes: u32,
    /// Untracked now, as in 0.x (issue #310).
    pub untracked: u32,
    /// 0.x's active account became v1's switch (v1 had none).
    pub active_seeded: bool,
}

/// Register the 0.x accounts in one transaction (Plan O). A new account's
/// `first_seen_at` is its 0.x creation time, else `now_ms`; an account v1
/// already has keeps its own unless 0.x created it earlier (issue #190). A
/// login id fills v1's only when v1 has none (the tracker's COALESCE rule);
/// one another v1 account already has is left out, never an error, and
/// counted as a clash only by the run that registers the account, so a rerun
/// does not report it again.
///
/// An account 0.x has untracked becomes untracked unless the user set
/// tracking in v1 (`tracked_chosen`); the import never turns tracking on.
/// 0.x's active account becomes v1's switch (`active_account`) when v1 has
/// none, so a switch made in v1, before the import or since, wins (issue
/// #310).
pub fn import_accounts(
    conn: &mut Connection,
    accounts: &[ImportedAccount],
    now_ms: i64,
) -> Result<AccountImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = AccountImportCounts::default();
    for account in accounts {
        let first_seen_at = account.created_at_ms.unwrap_or(now_ms);
        let added = tx.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, ?2)
             ON CONFLICT (username) DO NOTHING",
            params![account.username, first_seen_at],
        )? == 1;
        if added {
            counts.added += 1;
        } else {
            counts.unchanged += 1;
            if let Some(created_at) = account.created_at_ms {
                tx.execute(
                    "UPDATE accounts SET first_seen_at = ?2
                     WHERE username = ?1 AND first_seen_at > ?2",
                    params![account.username, created_at],
                )?;
            }
        }
        if !account.tracked {
            counts.untracked += tx.execute(
                "UPDATE accounts SET tracked = 0
                 WHERE username = ?1 AND tracked = 1 AND tracked_chosen = 0",
                [&account.username],
            )? as u32;
        }
        let Some(login_id) = account.login_id else {
            continue;
        };
        let stored: Option<i64> = tx.query_row(
            "SELECT login_id FROM accounts WHERE username = ?1",
            [&account.username],
            |row| row.get(0),
        )?;
        if stored.is_some() {
            // v1's login id is never replaced.
            continue;
        }
        let taken: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM accounts WHERE login_id = ?1)",
            [login_id],
            |row| row.get(0),
        )?;
        if taken {
            if added {
                counts.login_id_clashes += 1;
            }
            continue;
        }
        tx.execute(
            "UPDATE accounts SET login_id = ?2 WHERE username = ?1 AND login_id IS NULL",
            params![account.username, login_id],
        )?;
        counts.login_ids_filled += 1;
    }
    counts.active_seeded = seed_active_account(&tx, accounts)?;
    tx.commit()?;
    Ok(counts)
}

/// Store 0.x's active account as v1's switch when v1 has none (0.x keeps
/// one; with several, the one made active last). Returns whether it did.
fn seed_active_account(tx: &Transaction, accounts: &[ImportedAccount]) -> Result<bool, StoreError> {
    let mut active: Option<&ImportedAccount> = None;
    for account in accounts {
        let Some(since) = account.active_since_ms else {
            continue;
        };
        let newer = match active {
            None => true,
            Some(found) => found.active_since_ms.is_some_and(|found| since > found),
        };
        if newer {
            active = Some(account);
        }
    }
    let Some(active) = active else {
        return Ok(false);
    };
    // A stored `null` (settings saved before any switch) is no switch.
    if crate::accounts::active_account(tx)?.is_some() {
        return Ok(false);
    }
    let account_id: i64 = tx.query_row(
        "SELECT id FROM accounts WHERE username = ?1",
        [&active.username],
        |row| row.get(0),
    )?;
    let value = serde_json::to_string(&crate::accounts::ActiveAccount {
        account_id,
        switched_at_ms: active.active_since_ms.unwrap_or(0),
    })
    .map_err(|error| StoreError::Corrupt(error.to_string()))?;
    tx.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![crate::accounts::ACTIVE_ACCOUNT, value],
    )?;
    Ok(true)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportCounts {
    pub added: u32,
    pub updated: u32,
    pub unchanged: u32,
    /// The token or MTGO match ID belongs to a live (`source = 'log'`) row,
    /// or the token to a manual one (never replaced either).
    pub skipped_live: u32,
    /// 0.x shared replay links stored on a v1 match that had none.
    pub replay_shares_added: u32,
    /// 0.x shared replay links with no v1 match to hold them: no row has the
    /// match's token (v1 tracked it live under another token), or that row
    /// holds another link.
    pub replay_shares_unplaced: u32,
}

/// What happened to one imported match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Added,
    Updated,
    Unchanged,
    SkippedLive,
}

/// The name 0.x gave the local player of a manual match when MTGO gave no
/// username (`CreateManualMatch`). The importer maps it to the real account
/// when 0.x has one (issue #162); a rerun moves matches stored on it, and the
/// account goes once nothing uses it.
pub const LEGACY_LOCAL_PLACEHOLDER: &str = "You";

/// Write one batch in one transaction (spec section 5 of the importer spec).
pub fn import_matches(
    conn: &mut Connection,
    matches: &[ImportedMatch],
    now_ms: i64,
) -> Result<ImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = ImportCounts::default();
    for incoming in matches {
        match import_one(&tx, incoming, now_ms)? {
            Change::Added => counts.added += 1,
            Change::Updated => counts.updated += 1,
            Change::Unchanged => counts.unchanged += 1,
            Change::SkippedLive => counts.skipped_live += 1,
        }
        match insert_replay_share(&tx, incoming)? {
            ShareOutcome::Added => counts.replay_shares_added += 1,
            ShareOutcome::Unplaced => counts.replay_shares_unplaced += 1,
            ShareOutcome::None | ShareOutcome::Held => {}
        }
    }
    drop_unused_placeholder(&tx)?;
    tx.commit()?;
    Ok(counts)
}

pub(crate) fn import_one(
    tx: &Transaction,
    incoming: &ImportedMatch,
    now_ms: i64,
) -> Result<Change, StoreError> {
    if let Some(live_token) = live_row_token(tx, &incoming.token, incoming.mtgo_match_id)? {
        // Live wins: only the 0.x note is kept, on the live row.
        insert_note(tx, &live_token, incoming.note.as_deref())?;
        return Ok(Change::SkippedLive);
    }
    // A manual match (Plan L) is the player's own record: like a live row, it
    // is never replaced. A 0.x manual match imported earlier only has its
    // empty games filled; a tracked 0.x match never has a manual match's
    // token (a fresh UUID), so it is skipped.
    if crate::write::is_manual_token(tx, &incoming.token)? {
        let Some(details) = &incoming.manual else {
            return Ok(Change::SkippedLive);
        };
        let moved = move_off_placeholder(tx, incoming, now_ms)?;
        let change = match fill_manual_games(tx, incoming, details)? {
            Change::Unchanged if moved => Change::Updated,
            other => other,
        };
        insert_note(tx, &incoming.token, incoming.note.as_deref())?;
        return Ok(change);
    }
    // A match the player corrected (edit a match spec section 2) keeps the
    // fix: its row, games and detail are not rewritten. Only a 0.x note
    // missing in v1 is added.
    if crate::write::corrected_match_id(tx, &incoming.token)?.is_some() {
        insert_note(tx, &incoming.token, incoming.note.as_deref())?;
        return Ok(Change::Unchanged);
    }
    let wanted = storable(incoming);
    // The 0.x copy wins over a history import of the same match (Plan M):
    // one with this token becomes the 0.x match below, and one with another
    // token is replaced once the 0.x match is stored.
    let mut replaces_history = false;
    let mut history_copies: Vec<i64> = Vec::new();
    for (id, token) in history_rows(tx, &wanted.token, wanted.mtgo_match_id)? {
        // 0.x's own deck link wins over the history import's (ruling I1):
        // the copy's goes, so the deck import writes 0.x's.
        if incoming.deck_linked {
            tx.execute(
                "UPDATE matches SET deck_version_id = NULL WHERE id = ?1",
                [id],
            )?;
        }
        if token == wanted.token {
            forget_history_import(tx, &wanted)?;
            replaces_history = true;
        } else {
            history_copies.push(id);
        }
    }
    let account_id = registry_id(tx, "accounts", &wanted.local_username, now_ms)?;
    let opponent_id = match &wanted.opponent {
        Some(name) => Some(registry_id(tx, "opponents", name, now_ms)?),
        None => None,
    };
    let (match_id, change) = match stored_match(tx, &wanted.token)? {
        None => {
            let match_id = insert_match(tx, &wanted, account_id, opponent_id)?;
            insert_games(tx, match_id, &wanted.games)?;
            (match_id, Change::Added)
        }
        Some((match_id, stored)) if stored == wanted && !replaces_history => {
            (match_id, Change::Unchanged)
        }
        Some((match_id, _)) => {
            // A stored imported row: brought up to date, and made manual when
            // 0.x has it as a manual match (an import before Plan L stored it
            // as plain imported).
            update_match(tx, match_id, &wanted, account_id, opponent_id)?;
            if wanted.manual.is_some() {
                tx.execute(
                    "UPDATE matches SET source = 'manual' WHERE id = ?1 AND source = 'imported'",
                    [match_id],
                )?;
                // A manual match keeps its own records, never a 0.x
                // import's detail (Plan O): its games are written again below.
                crate::game_sync::clear_match_detail(tx, match_id)?;
            }
            tx.execute("DELETE FROM games WHERE match_id = ?1", [match_id])?;
            insert_games(tx, match_id, &wanted.games)?;
            (match_id, Change::Updated)
        }
    };
    if let Some(details) = &wanted.manual
        && !matches!(change, Change::Unchanged)
    {
        for detail in details {
            insert_manual_detail(tx, match_id, detail)?;
        }
        mark_import_revision(tx, match_id)?;
    }
    insert_note(tx, &incoming.token, incoming.note.as_deref())?;
    // After the 0.x note, so a history copy's note moves only when 0.x has none.
    for copy_id in history_copies {
        crate::write::hand_over_records(tx, copy_id, match_id)?;
        tx.execute(
            "DELETE FROM matches WHERE id = ?1 AND source = 'history'",
            [copy_id],
        )?;
    }
    Ok(change)
}

/// A match pulled from the cloud (07 spec section 10), read from its bundle:
/// the match as the 0.x import takes one, its games' detail, and the links
/// the bundle names by cross-device ids.
#[derive(Debug, Clone, PartialEq)]
pub struct PulledMatch {
    /// The game the bundle is from (absent in the bundle: MTGO).
    pub platform: sync_bundles::BundlePlatform,
    /// Arena and Endstep: the bundle's raw `sources`.
    pub sources: Option<sync_bundles::MatchSources>,
    /// The local seat's Arena fields (`display_name`, `arena_user_id`).
    pub local_player_fields: sync_bundles::match_bundle::ArenaPlayerFields,
    /// The opponent seat's Arena fields.
    pub opponent_player_fields: sync_bundles::match_bundle::ArenaPlayerFields,
    /// Endstep: the match's ranked result (`match.endstep_ranked`).
    pub endstep_ranked: Option<sync_bundles::match_bundle::EndstepRankedFields>,
    /// Endstep manual match: the cards its manual detail names, by ref.
    /// The bundle reader numbers each card by its place here plus one (an
    /// Endstep bundle holds no catalog id), and the import maps each to
    /// this device's own (`local_manual_cards`). Empty otherwise.
    pub endstep_manual_cards: Vec<sync_bundles::EndstepCardRef>,
    /// Arena and Endstep: the match's kind (`match.game_structure`, `bo1`
    /// or `bo3`), which the deck pages filter on. `None` for MTGO.
    pub game_structure: Option<String>,
    pub imported: ImportedMatch,
    pub detail: crate::imported_detail::ImportedDetail,
    /// The deck version played: the deck's MTGO id and the signature.
    pub deck: Option<(String, String)>,
    /// The run it is in (`league_client_id`).
    pub league_client_id: Option<String>,
    /// Imported from MTGO's match history on the device that played it (0.x
    /// `imported`): stored as a history import here too.
    pub history: bool,
    /// The bundle's archetype seats (archetypes spec section 14).
    pub archetypes: Vec<sync_bundles::match_bundle::ArchetypeSide>,
    /// The bundle's player fields (league self-management spec 6.3),
    /// applied by client id after the import (`sync_player`).
    pub player: sync_bundles::PulledPlayerFields,
}

/// What importing one pulled match did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledMatchImport {
    /// The token it is stored under: its own, or the live row's that won.
    pub token: Option<String>,
    /// Stored as this device's copy (added or updated), not left to a live
    /// or manual row.
    pub stored: bool,
}

/// Whether a pulled match is a tracked Arena or Endstep match whose source
/// did not travel: it is never stored (no half import).
pub(crate) fn lacks_source(pulled: &PulledMatch) -> bool {
    store_platform(pulled.platform) != crate::Platform::Mtgo
        && pulled.sources.is_none()
        && pulled.imported.manual.is_none()
}

/// Import one pulled match like a 0.x match (live wins over imported, as
/// Plan F): its row and games, its detail, its deck version (made on a
/// known deck when this device lacks it, as 0.x did) and its run. A
/// complete match is recorded as reported: the device that played it
/// reported it (0.x marks it submitted).
///
/// An Arena or Endstep match (cross-device sync spec section 7) is filed
/// under an account and opponent of its platform, its raw source is kept
/// in `synced_sources` and its game rows are written by the same writers
/// the local game syncs use. One without a source (a tracked match whose
/// facts did not travel) is not stored: no half import.
pub(crate) fn import_pulled_match(
    tx: &Transaction,
    pulled: &PulledMatch,
    now_ms: i64,
) -> Result<PulledMatchImport, StoreError> {
    let not_stored = PulledMatchImport {
        token: None,
        stored: false,
    };
    let platform = store_platform(pulled.platform);
    if lacks_source(pulled) {
        tracing::warn!(
            platform = platform.as_str(),
            "Sync pull: a tracked match came without its source; not stored."
        );
        return Ok(not_stored);
    }
    // A bundle with no games names no player: the match is this device's
    // account's of its platform (0.x gave decks without one the active
    // account).
    let mut filled = pulled.imported.clone();
    if filled.local_username.trim().is_empty() {
        // The account of its platform with the bundle's user id first, then
        // the current one.
        let account = match account_of_user_id(tx, platform, pulled)? {
            Some(account) => Some(account),
            None => match crate::accounts::current_account_id(tx, platform)? {
                Some(id) => tx
                    .prepare_cached("SELECT username FROM accounts WHERE id = ?1")?
                    .query_row([id], |row| row.get(0))
                    .optional()?,
                None => None,
            },
        };
        let Some(account) = account else {
            return Ok(not_stored);
        };
        filled.local_username = account;
    } else if platform != crate::Platform::Mtgo {
        let Some(account) = pulled_platform_account(tx, platform, pulled, now_ms)? else {
            return Ok(not_stored);
        };
        filled.local_username = account;
    }
    if platform == crate::Platform::Endstep
        && let Some(details) = &mut filled.manual
    {
        local_manual_cards(tx, details, &pulled.endstep_manual_cards, now_ms)?;
    }
    // The opponent is made with its platform and display name before
    // `import_one`, whose `registry_id` then finds it (it would otherwise
    // insert an MTGO opponent). It is written even when a live row wins
    // below: the same opponent, so nothing is lost.
    if platform != crate::Platform::Mtgo
        && let Some(opponent) = &filled.opponent
    {
        pulled_platform_opponent(tx, platform, opponent, pulled, now_ms)?;
    }
    let incoming = &filled;
    let change = import_one(tx, incoming, now_ms)?;
    let token = match live_row_token(tx, &incoming.token, incoming.mtgo_match_id)? {
        Some(live) => Some(live),
        None => tx
            .prepare_cached("SELECT token FROM matches WHERE token = ?1")?
            .query_row([&incoming.token], |row| row.get::<_, String>(0))
            .optional()?,
    };
    let stored = matches!(change, Change::Added | Change::Updated);
    if stored {
        // The version first: the detail's imported postboard rule reads it.
        let version = match &pulled.deck {
            Some((deck_mtgo_id, signature)) => {
                match local_signature(tx, platform, signature, now_ms)? {
                    Some(signature) => pulled_deck_version(tx, deck_mtgo_id, &signature, now_ms)?,
                    None => None,
                }
            }
            None => None,
        };
        tx.prepare_cached("UPDATE matches SET deck_version_id = ?2 WHERE token = ?1")?
            .execute(params![incoming.token, version])?;
        // An Arena or Endstep deck pulled before this device had an account
        // of its platform is filed under the account that played it.
        if platform != crate::Platform::Mtgo
            && let Some(version) = version
        {
            tx.prepare_cached(
                "UPDATE decks SET account_id = (SELECT account_id FROM matches WHERE token = ?1)
                 WHERE id = (SELECT deck_id FROM deck_versions WHERE id = ?2)
                   AND account_id IS NULL
                   AND (pulled_platform IS NULL OR pulled_platform = ?3)",
            )?
            .execute(params![incoming.token, version, platform.as_str()])?;
        }
        match &pulled.sources {
            Some(sources) if platform != crate::Platform::Mtgo => {
                crate::imported_detail::store_sources(tx, &incoming.token, sources, now_ms)?;
            }
            _ => {
                crate::imported_detail::store_detail(tx, &incoming.token, &pulled.detail)?;
            }
        }
        // Its kind, which the deck pages filter on (MTGO's never travels).
        if platform != crate::Platform::Mtgo
            && let Some(kind) = &pulled.game_structure
        {
            tx.prepare_cached("UPDATE matches SET game_structure = ?2 WHERE token = ?1")?
                .execute(params![incoming.token, kind])?;
        }
        if platform == crate::Platform::Endstep
            && let Some(ranked) = &pulled.endstep_ranked
        {
            crate::endstep_sync::upsert_ranked(tx, &pulled_ranked(&incoming.token, ranked))?;
        }
        if pulled.history {
            tx.prepare_cached(
                "UPDATE matches SET source = 'history' WHERE token = ?1 AND source = 'imported'",
            )?
            .execute([&incoming.token])?;
        }
    }
    // A copy's league link applies on every pull, so a pull that changed
    // only the link is not lost as Unchanged (league self-management spec
    // 6.3); a live or manual row's link is its own.
    if stored || is_copy(tx, &incoming.token)? {
        link_pulled(tx, &incoming.token, pulled.league_client_id.as_deref())?;
    }
    if incoming.state == "complete"
        && let Some(token) = &token
    {
        crate::outbox::record_sent(tx, crate::outbox::OutboxKind::MatchReport, token, now_ms)?;
    }
    Ok(PulledMatchImport { token, stored })
}

/// The store's platform of a bundle's.
pub(crate) fn store_platform(platform: sync_bundles::BundlePlatform) -> crate::Platform {
    match platform {
        sync_bundles::BundlePlatform::Mtgo => crate::Platform::Mtgo,
        sync_bundles::BundlePlatform::Arena => crate::Platform::Arena,
        sync_bundles::BundlePlatform::Endstep => crate::Platform::Endstep,
    }
}

/// The username a pulled Arena or Endstep match is filed under: the
/// account of its platform holding the bundle's user id (a rename here is
/// never undone by an older bundle), else the bundle's account, created or
/// matched by `upsert_platform_account`. `None` when an account of another
/// platform holds the name.
fn pulled_platform_account(
    tx: &Transaction,
    platform: crate::Platform,
    pulled: &PulledMatch,
    now_ms: i64,
) -> Result<Option<String>, StoreError> {
    let fields = &pulled.local_player_fields;
    if let Some(held) = account_of_user_id(tx, platform, pulled)? {
        return Ok(Some(held));
    }
    let username = crate::arena::platform_account_username(tx, &pulled.imported.local_username)?;
    crate::arena::upsert_platform_account(
        tx,
        &crate::arena::PlatformAccount {
            platform,
            username: username.clone(),
            user_id: fields.arena_user_id.clone(),
            display_name: fields.display_name.clone(),
            seen_at_ms: now_ms,
        },
    )?;
    let stored_platform: Option<String> = tx
        .prepare_cached("SELECT platform FROM accounts WHERE username = ?1")?
        .query_row([&username], |row| row.get(0))
        .optional()?;
    if stored_platform.as_deref() != Some(platform.as_str()) {
        tracing::warn!(
            platform = platform.as_str(),
            "Sync pull: an account of another platform holds the match's account name; not stored."
        );
        return Ok(None);
    }
    Ok(Some(username))
}

/// The username of the account of `platform` holding the bundle's local
/// `arena_user_id`, if any.
fn account_of_user_id(
    tx: &Transaction,
    platform: crate::Platform,
    pulled: &PulledMatch,
) -> Result<Option<String>, StoreError> {
    let Some(user_id) = &pulled.local_player_fields.arena_user_id else {
        return Ok(None);
    };
    Ok(tx
        .prepare_cached("SELECT username FROM accounts WHERE arena_user_id = ?1 AND platform = ?2")?
        .query_row(params![user_id, platform], |row| row.get(0))
        .optional()?)
}

/// The opponent `username` of a pulled Arena or Endstep match, created
/// with its platform and the bundle's display name (kept when the bundle
/// has none). An opponent of another platform holding the name is left
/// alone.
fn pulled_platform_opponent(
    tx: &Transaction,
    platform: crate::Platform,
    username: &str,
    pulled: &PulledMatch,
    now_ms: i64,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO opponents (username, first_seen_at, platform, display_name)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (username) DO UPDATE SET
             display_name = COALESCE(excluded.display_name, opponents.display_name)
         WHERE opponents.platform = ?3",
    )?
    .execute(params![
        username,
        now_ms,
        platform,
        pulled.opponent_player_fields.display_name
    ])?;
    Ok(())
}

/// A pulled deck version signature as this device stores it: an Endstep
/// bundle's portable signature (`endstep:` and card refs) becomes the
/// local one, each card at this device's catalog id; any other signature
/// is kept as it is. `None` (warned) for an `endstep:` signature that does
/// not read: the match links no deck version rather than store junk.
pub(crate) fn local_signature(
    tx: &Transaction,
    platform: crate::Platform,
    signature: &str,
    now_ms: i64,
) -> Result<Option<String>, StoreError> {
    if platform != crate::Platform::Endstep {
        return Ok(Some(signature.to_string()));
    }
    let Some(cards) = sync_bundles::endstep_cards::read_endstep_signature(signature) else {
        if signature.starts_with(sync_bundles::endstep_cards::ENDSTEP_SIGNATURE_PREFIX) {
            tracing::warn!("Sync pull: an Endstep deck signature did not read; no deck version.");
            return Ok(None);
        }
        return Ok(Some(signature.to_string()));
    };
    let mut rows = Vec::new();
    for card in cards {
        rows.push(decks::DeckCard {
            catalog_id: crate::endstep_cards::catalog_for_ref(tx, &card.card, now_ms)?,
            quantity: card.quantity,
            sideboard: card.sideboard,
        });
    }
    Ok(Some(decks::signature(&rows)))
}

/// A pulled Endstep manual match's detail at this device's catalog ids:
/// each card numbered by its place in `cards` (plus one) becomes the
/// local id of that ref, made when new. A number with no ref is left out.
fn local_manual_cards(
    tx: &Transaction,
    details: &mut [ManualGameDetail],
    cards: &[sync_bundles::EndstepCardRef],
    now_ms: i64,
) -> Result<(), StoreError> {
    let mut local: Vec<u32> = Vec::new();
    for card in cards {
        local.push(crate::endstep_cards::catalog_for_ref(tx, card, now_ms)?);
    }
    let map = |number: u32| -> Option<u32> {
        let place = usize::try_from(number).ok()?.checked_sub(1)?;
        local.get(place).copied()
    };
    let map_ids = |ids: &[u32]| -> Vec<u32> { ids.iter().filter_map(|id| map(*id)).collect() };
    let map_counts = |counts: &[crate::manual::CardCount]| -> Vec<crate::manual::CardCount> {
        counts
            .iter()
            .filter_map(|count| {
                Some(crate::manual::CardCount {
                    catalog_id: map(count.catalog_id)?,
                    quantity: count.quantity,
                })
            })
            .collect()
    };
    for detail in details.iter_mut() {
        detail.starting_hand = detail.starting_hand.as_deref().map(map_ids);
        detail.bottomed = map_ids(&detail.bottomed);
        detail.mulligans = detail.mulligans.iter().map(|hand| map_ids(hand)).collect();
        detail.postboard_main = detail.postboard_main.as_deref().map(map_counts);
        detail.reveals = map_counts(&detail.reveals);
    }
    Ok(())
}

/// A bundle's Endstep ranked result as stored against the match `token`.
fn pulled_ranked(
    token: &str,
    ranked: &sync_bundles::match_bundle::EndstepRankedFields,
) -> crate::endstep_sync::EndstepRanked {
    crate::endstep_sync::EndstepRanked {
        match_token: token.to_string(),
        status: ranked.status.clone(),
        format_id: ranked.format_id.clone(),
        rating_before: ranked.rating_before,
        rating_after: ranked.rating_after,
        score_before: ranked.score_before,
        score_after: ranked.score_after,
        delta: ranked.delta,
        rd_before: ranked.rd_before,
        rd_after: ranked.rd_after,
        rank: ranked.rank.clone(),
    }
}

/// 0.x `MatchBundleImporter::resolveDeckVersionId`: the deck by MTGO id, the
/// version by signature, made (dated now) when the deck is known but the
/// version is not; `None` without the deck.
fn pulled_deck_version(
    tx: &Transaction,
    deck_mtgo_id: &str,
    signature: &str,
    now_ms: i64,
) -> Result<Option<i64>, StoreError> {
    let deck_id: Option<i64> = tx
        .prepare_cached("SELECT id FROM decks WHERE mtgo_id = ?1")?
        .query_row([deck_mtgo_id], |row| row.get(0))
        .optional()?;
    let Some(deck_id) = deck_id else {
        return Ok(None);
    };
    tx.prepare_cached(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (deck_id, signature) DO NOTHING",
    )?
    .execute(params![deck_id, signature, now_ms])?;
    let catalog_ids: Vec<u32> = decks::decode_signature(signature)
        .iter()
        .map(|card| card.catalog_id)
        .collect();
    crate::cards::ensure_id_stubs(tx, &catalog_ids)?;
    Ok(tx
        .prepare_cached("SELECT id FROM deck_versions WHERE deck_id = ?1 AND signature = ?2")?
        .query_row(params![deck_id, signature], |row| row.get(0))
        .optional()?)
}

/// Whether `token` is a pulled or 0.x copy (`imported` or `history`).
fn is_copy(tx: &Transaction, token: &str) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches
                            WHERE token = ?1 AND source IN ('imported', 'history'))",
        )?
        .query_row([token], |row| row.get(0))?)
}

/// Link a pulled copy to the run its bundle names, as 0.x did, never over
/// the player's own choice: a match out of every league (`no_league`) or
/// edited here keeps its league (a newer edit comes through the player
/// fields, `sync_player`). A log run of this device (spec 6.3) must pass
/// the move rules (4.1), judged as `sync_player`'s links are: a hidden copy
/// as if it were listed (`move_refusal_judged_listed`). A refused link is
/// dropped and logged.
fn link_pulled(tx: &Transaction, token: &str, client_id: Option<&str>) -> Result<(), StoreError> {
    let Some(league_id) = pulled_league_id(tx, client_id)? else {
        return Ok(());
    };
    let row: Option<(i64, bool)> = tx
        .prepare_cached(
            "SELECT m.id, l.source = 'log' FROM matches m, leagues l
             WHERE m.token = ?1 AND l.id = ?2",
        )?
        .query_row(params![token, league_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .optional()?;
    let Some((match_id, log_run)) = row else {
        return Ok(());
    };
    if log_run
        && let Some(refusal) = crate::leagues::move_refusal_judged_listed(tx, match_id, league_id)?
    {
        tracing::warn!(
            token,
            ?refusal,
            "a pulled league link to a log run was refused"
        );
        return Ok(());
    }
    tx.prepare_cached(
        "UPDATE matches SET league_id = ?2, league_linked = 1
         WHERE id = ?1 AND (league_linked = 0 OR league_id = ?2)
           AND no_league = 0 AND edited_at IS NULL",
    )?
    .execute(params![match_id, league_id])?;
    Ok(())
}

/// The run a league client id names: a pulled, 0.x or hand-made run, or
/// (league self-management spec 6.3) a log run of this device; never a
/// removed one. The run that stores the id (`sync_client_id`) first, else
/// (an id from a device that has not stored its ids yet) the run with its
/// token and start (`pulled_league_id_by_start`).
pub(crate) fn pulled_league_id(
    tx: &Transaction,
    client_id: Option<&str>,
) -> Result<Option<i64>, StoreError> {
    let Some(client_id) = client_id else {
        return Ok(None);
    };
    let stored: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM leagues WHERE sync_client_id = ?1
                 AND removed_at IS NULL
                 AND (source IN ('imported', 'manual')
                      OR (source = 'log' AND detached_at IS NULL))
             ORDER BY source = 'log', id LIMIT 1",
        )?
        .query_row([client_id], |row| row.get(0))
        .optional()?;
    if stored.is_some() {
        return Ok(stored);
    }
    pulled_league_id_by_start(tx, client_id)
}

/// `pulled_league_id` by the client id's token and start
/// (`{token}_{Ymd\THis\Z}`, split at the fixed-shape suffix as 0.x does).
fn pulled_league_id_by_start(tx: &Transaction, client_id: &str) -> Result<Option<i64>, StoreError> {
    let Some((token, started_at)) = split_league_client_id(client_id) else {
        return Ok(None);
    };
    Ok(tx
        .prepare_cached(
            "SELECT id FROM leagues WHERE token = ?1 AND started_at / 1000 = ?2 / 1000
                 AND removed_at IS NULL
                 AND (source IN ('imported', 'manual')
                      OR (source = 'log' AND detached_at IS NULL))
             ORDER BY source = 'log', id LIMIT 1",
        )?
        .query_row(params![token, started_at], |row| row.get(0))
        .optional()?)
}

/// A league client id's token and start (UTC ms).
pub fn split_league_client_id(client_id: &str) -> Option<(String, i64)> {
    let (token, stamp) = client_id.rsplit_once('_')?;
    if token.is_empty() || stamp.len() != 16 {
        return None;
    }
    let started = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ").ok()?;
    Some((token.to_string(), started.and_utc().timestamp_millis()))
}

/// The history imports (Plan M) of this match: rows the history importer
/// stored (`source = 'history'`) with this token or this MTGO match ID.
fn history_rows(
    tx: &Transaction,
    token: &str,
    mtgo_match_id: Option<i64>,
) -> Result<Vec<(i64, String)>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT id, token FROM matches
         WHERE source = 'history'
           AND (token = ?1 OR (?2 IS NOT NULL AND mtgo_match_id = ?2))
         ORDER BY id",
    )?;
    let rows = stmt.query_map(params![token, mtgo_match_id], |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

/// The history import with the 0.x match's own token becomes the 0.x match
/// (`source = 'imported'`), so "Delete all imported" no longer removes it,
/// and the history's game counts give way to 0.x's: its score when the 0.x
/// match has no games, none otherwise (its game rows are the score).
fn forget_history_import(tx: &Transaction, wanted: &ImportedMatch) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE matches SET source = 'imported', games_won_imported = ?2, games_lost_imported = ?3
         WHERE token = ?1 AND source = 'history'",
        params![
            wanted.token,
            wanted.games_won_imported,
            wanted.games_lost_imported
        ],
    )?;
    Ok(())
}

/// A manual match an earlier import stored on the "You" placeholder moves to
/// the account 0.x now gives it (issue #162). Still on the placeholder means
/// the player never gave it another account in v1. Returns whether it moved.
fn move_off_placeholder(
    tx: &Transaction,
    incoming: &ImportedMatch,
    now_ms: i64,
) -> Result<bool, StoreError> {
    if incoming.local_username == LEGACY_LOCAL_PLACEHOLDER {
        return Ok(false);
    }
    let on_placeholder: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches m JOIN accounts a ON a.id = m.account_id
                 WHERE m.token = ?1 AND m.source = 'manual' AND a.username = ?2)",
        )?
        .query_row(params![incoming.token, LEGACY_LOCAL_PLACEHOLDER], |row| {
            row.get(0)
        })?;
    if !on_placeholder {
        return Ok(false);
    }
    let account_id = registry_id(tx, "accounts", &incoming.local_username, now_ms)?;
    tx.prepare_cached("UPDATE matches SET account_id = ?2 WHERE token = ?1")?
        .execute(params![incoming.token, account_id])?;
    Ok(true)
}

/// Delete the "You" placeholder account once no row names it and it has no
/// login id. Every table that points at `accounts` restricts the delete,
/// except the event overlay's derived tournament tables, which cascade (the
/// placeholder never has rows there: they come only from this device's log).
fn drop_unused_placeholder(tx: &Transaction) -> Result<(), StoreError> {
    tx.prepare_cached(
        "DELETE FROM accounts WHERE username = ?1 AND login_id IS NULL
           AND NOT EXISTS (SELECT 1 FROM matches WHERE account_id = accounts.id)
           AND NOT EXISTS (SELECT 1 FROM decks WHERE account_id = accounts.id)
           AND NOT EXISTS (SELECT 1 FROM leagues WHERE account_id = accounts.id)
           AND NOT EXISTS (SELECT 1 FROM tournaments WHERE account_id = accounts.id)
           AND NOT EXISTS (SELECT 1 FROM drafts WHERE account_id = accounts.id)",
    )?
    .execute([LEGACY_LOCAL_PLACEHOLDER])?;
    Ok(())
}

/// A rerun of a 0.x manual match already stored as manual: each game v1 has
/// no record for (no hand, sideboarding or reveals in its rows) gets 0.x's,
/// with its mulligan count when it has a hand. A game with a record is never
/// touched. Once the player edited the match in v1 (`edited_in_v1`) the rerun
/// leaves every game alone: a game cleared in v1 has no rows, and filling it
/// again would undo the player's edit. Updated when a game was filled.
fn fill_manual_games(
    tx: &Transaction,
    incoming: &ImportedMatch,
    details: &[ManualGameDetail],
) -> Result<Change, StoreError> {
    let (match_id, edited): (i64, bool) = tx.query_row(
        "SELECT id, revision > derived_version FROM matches WHERE token = ?1 AND source = 'manual'",
        [&incoming.token],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if edited {
        return Ok(Change::Unchanged);
    }
    let mut filled = false;
    for detail in details {
        let has_game: bool = tx
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM games WHERE match_id = ?1 AND game_number = ?2)",
            )?
            .query_row(params![match_id, detail.game_number], |row| row.get(0))?;
        if !has_game || !insert_manual_detail(tx, match_id, detail)? {
            continue;
        }
        filled = true;
        if detail.starting_hand.is_some() {
            let mulligans = incoming
                .games
                .iter()
                .find(|game| game.game_number == detail.game_number)
                .and_then(|game| game.local_mulligans)
                .unwrap_or(0);
            tx.execute(
                "UPDATE games SET local_mulligans = ?3 WHERE match_id = ?1 AND game_number = ?2",
                params![match_id, detail.game_number, mulligans],
            )?;
        }
    }
    if !filled {
        return Ok(Change::Unchanged);
    }
    tx.execute(
        "UPDATE matches SET revision = revision + 1, derived_version = revision + 1
         WHERE id = ?1",
        [match_id],
    )?;
    Ok(Change::Updated)
}

/// Mark a manual match's revision as the import's own (`edited_in_v1`): on a
/// manual row no derivation ever runs, so `derived_version` holds the
/// revision the 0.x import last wrote. Every v1 edit bumps `revision` only
/// (`manual::bump_revision`), so `revision > derived_version` means the
/// player edited the match in v1 since.
fn mark_import_revision(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE matches SET derived_version = revision WHERE id = ?1 AND source = 'manual'",
        [match_id],
    )?;
    Ok(())
}

/// Store one game's 0.x hand, sideboarding and reveals as the manual edits
/// write them (`manual::set_hand`, `set_sideboard`, `set_reveals`), unless
/// the game already has a record. A detail with nothing recorded is not
/// stored. Returns whether anything was written.
fn insert_manual_detail(
    tx: &Transaction,
    match_id: i64,
    detail: &ManualGameDetail,
) -> Result<bool, StoreError> {
    if detail.is_empty() {
        return Ok(false);
    }
    if !crate::manual::stored_detail(tx, match_id, detail.game_number)?.is_empty() {
        return Ok(false);
    }
    crate::manual::write_detail(tx, match_id, detail)
}

/// Store a non-blank 0.x note on the match `token`, unless it has a note
/// already (a note written in v1 is never overwritten). 0.x allowed at most
/// 5000 characters, as `matches.notes` does; a longer one keeps its first
/// 5000.
fn insert_note(tx: &Transaction, token: &str, note: Option<&str>) -> Result<(), StoreError> {
    let Some(note) = note else {
        return Ok(());
    };
    if note.trim().is_empty() {
        return Ok(());
    }
    let note: String = note
        .chars()
        .take(crate::match_edits::MAX_NOTES_CHARS)
        .collect();
    tx.execute(
        "UPDATE matches SET notes = ?2 WHERE token = ?1 AND notes IS NULL",
        params![token, note],
    )?;
    Ok(())
}

/// What happened to a 0.x match's shared replay link.
enum ShareOutcome {
    /// The match has none.
    None,
    Added,
    /// The v1 match holds this link already (a rerun).
    Held,
    /// No v1 match to hold it (`ImportCounts::replay_shares_unplaced`).
    Unplaced,
}

/// Store 0.x's shared replay link on the v1 match with the same token
/// (whatever its source: the link is keyed by the match token on
/// mymtgo.com), unless that match has a link already (input is never
/// overwritten).
fn insert_replay_share(
    tx: &Transaction,
    incoming: &ImportedMatch,
) -> Result<ShareOutcome, StoreError> {
    let Some(share) = &incoming.replay_share else {
        return Ok(ShareOutcome::None);
    };
    let stored = tx.execute(
        "UPDATE matches SET replay_share_uuid = ?2, replay_share_url = ?3, replay_shared_at = ?4
         WHERE token = ?1 AND replay_share_uuid IS NULL",
        params![incoming.token, share.uuid, share.url, share.shared_at_ms],
    )?;
    if stored > 0 {
        return Ok(ShareOutcome::Added);
    }
    let held: Option<Option<String>> = tx
        .prepare_cached("SELECT replay_share_uuid FROM matches WHERE token = ?1")?
        .query_row([&incoming.token], |row| row.get(0))
        .optional()?;
    Ok(match held {
        Some(Some(uuid)) if uuid == share.uuid => ShareOutcome::Held,
        _ => ShareOutcome::Unplaced,
    })
}

/// The token of the live (`source = 'log'`) row with this token or this MTGO
/// Match ID, if there is one.
pub(crate) fn live_row_token(
    tx: &Transaction,
    token: &str,
    mtgo_match_id: Option<i64>,
) -> Result<Option<String>, StoreError> {
    let by_token: bool = tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM matches WHERE token = ?1 AND source = 'log')",
        [token],
        |row| row.get(0),
    )?;
    if by_token {
        return Ok(Some(token.to_string()));
    }
    let Some(mtgo_match_id) = mtgo_match_id else {
        return Ok(None);
    };
    let by_match_id: Option<String> = tx
        .query_row(
            "SELECT token FROM matches WHERE mtgo_match_id = ?1 AND source = 'log'
             ORDER BY id LIMIT 1",
            [mtgo_match_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(by_match_id)
}

/// The match as it will be stored, so it compares equal to what is read back:
/// no note (`matches.notes` is input, never compared), games in `game_number`
/// order, and only the first game with each Game ID (`games` has
/// `UNIQUE (match_id, mtgo_game_id)`).
fn storable(incoming: &ImportedMatch) -> ImportedMatch {
    let mut games: Vec<ImportedGame> = Vec::new();
    for game in &incoming.games {
        let seen = game.mtgo_game_id.is_some()
            && games
                .iter()
                .any(|kept| kept.mtgo_game_id == game.mtgo_game_id);
        if !seen {
            games.push(game.clone());
        }
    }
    // A stable sort: games with the same number keep their order.
    games.sort_by_key(|game| game.game_number);
    // A manual match has no MTGO match id (0.x gave it `manual_*`).
    let mtgo_match_id = if incoming.manual.is_some() {
        None
    } else {
        incoming.mtgo_match_id
    };
    ImportedMatch {
        note: None,
        replay_share: None,
        games,
        mtgo_match_id,
        deck_linked: false,
        ..incoming.clone()
    }
}

/// The stored row for `token` and its games, read back in the shape of an
/// `ImportedMatch` (without a note), with the row id.
fn stored_match(tx: &Transaction, token: &str) -> Result<Option<(i64, ImportedMatch)>, StoreError> {
    let found = tx
        .query_row(
            "SELECT m.id, m.token, m.mtgo_match_id, a.username, o.username, m.format,
                 m.match_type, m.tournament_event_id, m.tournament_round, m.state, m.outcome,
                 m.started_at, m.ended_at, m.games_won_imported, m.games_lost_imported,
                 m.legacy_history
             FROM matches m
             JOIN accounts a ON a.id = m.account_id
             LEFT JOIN opponents o ON o.id = m.opponent_id
             WHERE m.token = ?1",
            [token],
            |row| {
                let stored = ImportedMatch {
                    token: row.get(1)?,
                    mtgo_match_id: row.get(2)?,
                    local_username: row.get(3)?,
                    opponent: row.get(4)?,
                    format: row.get(5)?,
                    match_type: row.get(6)?,
                    tournament_event_id: row.get(7)?,
                    tournament_round: row.get(8)?,
                    state: row.get(9)?,
                    outcome: row.get(10)?,
                    started_at_ms: row.get(11)?,
                    ended_at_ms: row.get(12)?,
                    note: None,
                    games: Vec::new(),
                    manual: None,
                    games_won_imported: row.get(13)?,
                    games_lost_imported: row.get(14)?,
                    deck_linked: false,
                    replay_share: None,
                    legacy_history: row.get(15)?,
                };
                Ok((row.get::<_, i64>(0)?, stored))
            },
        )
        .optional()?;
    let Some((match_id, mut stored)) = found else {
        return Ok(None);
    };
    stored.games = stored_games(tx, match_id)?;
    Ok(Some((match_id, stored)))
}

fn stored_games(tx: &Transaction, match_id: i64) -> Result<Vec<ImportedGame>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT mtgo_game_id, game_number, result, on_play, local_mulligans, opp_mulligans,
             local_die_roll, opp_die_roll, turn_count, started_at, ended_at
         FROM games WHERE match_id = ?1 ORDER BY game_number, id",
    )?;
    let rows = stmt.query_map([match_id], |row| {
        Ok(ImportedGame {
            mtgo_game_id: row.get(0)?,
            game_number: row.get(1)?,
            result: row.get(2)?,
            on_play: row.get(3)?,
            local_mulligans: row.get(4)?,
            opp_mulligans: row.get(5)?,
            local_die_roll: row.get(6)?,
            opp_die_roll: row.get(7)?,
            turn_count: row.get(8)?,
            started_at_ms: row.get(9)?,
            ended_at_ms: row.get(10)?,
        })
    })?;
    let mut games = Vec::new();
    for row in rows {
        games.push(row?);
    }
    Ok(games)
}

fn last_activity_ms(imported: &ImportedMatch) -> i64 {
    imported.ended_at_ms.unwrap_or(imported.started_at_ms)
}

/// The `legacy_history` a 0.x match is stored with: 0.x's flag, except on a
/// manual match, which the schema keeps at 0 (only `imported` and `history`
/// rows hold 1; 0.x never flags a manual match, but its rows are input).
fn stored_legacy_history(imported: &ImportedMatch) -> bool {
    imported.legacy_history && imported.manual.is_none()
}

fn insert_match(
    tx: &Transaction,
    imported: &ImportedMatch,
    account_id: i64,
    opponent_id: Option<i64>,
) -> Result<i64, StoreError> {
    tx.execute(
        "INSERT INTO matches (token, mtgo_match_id, account_id, opponent_id, source, format,
             match_type, tournament_event_id, tournament_round, state, outcome, started_at,
             ended_at, last_activity_at, needs_attention, attention_reason, revision, derived_version,
             games_won_imported, games_lost_imported, legacy_history)
         VALUES (?1, ?2, ?3, ?4, ?14, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, NULL, 1, 0, ?15,
             ?16, ?17)",
        params![
            imported.token,
            imported.mtgo_match_id,
            account_id,
            opponent_id,
            imported.format,
            imported.match_type,
            imported.tournament_event_id,
            imported.tournament_round,
            imported.state,
            imported.outcome,
            imported.started_at_ms,
            imported.ended_at_ms,
            last_activity_ms(imported),
            if imported.manual.is_some() {
                "manual"
            } else {
                "imported"
            },
            imported.games_won_imported,
            imported.games_lost_imported,
            stored_legacy_history(imported),
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Bring a stored 0.x or pulled row up to date. A corrected row
/// (`corrected_at` set) is never written: `import_one` returns before it,
/// and the WHERE refuses should that check ever be skipped.
fn update_match(
    tx: &Transaction,
    match_id: i64,
    imported: &ImportedMatch,
    account_id: i64,
    opponent_id: Option<i64>,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE matches SET mtgo_match_id = ?2, account_id = ?3, opponent_id = ?4, format = ?5,
             match_type = ?6, tournament_event_id = ?7, tournament_round = ?8, state = ?9,
             outcome = ?10, started_at = ?11, ended_at = ?12, last_activity_at = ?13,
             games_won_imported = ?14, games_lost_imported = ?15, legacy_history = ?16,
             revision = revision + 1
         WHERE id = ?1 AND source = 'imported' AND corrected_at IS NULL",
        params![
            match_id,
            imported.mtgo_match_id,
            account_id,
            opponent_id,
            imported.format,
            imported.match_type,
            imported.tournament_event_id,
            imported.tournament_round,
            imported.state,
            imported.outcome,
            imported.started_at_ms,
            imported.ended_at_ms,
            last_activity_ms(imported),
            imported.games_won_imported,
            imported.games_lost_imported,
            stored_legacy_history(imported),
        ],
    )?;
    Ok(())
}

/// 0.x does not store how a game ended, so every imported game's `end_reason`
/// is `unknown`.
fn insert_games(tx: &Transaction, match_id: i64, games: &[ImportedGame]) -> Result<(), StoreError> {
    let mut stmt = tx.prepare_cached(
        "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason, on_play,
             local_mulligans, opp_mulligans, local_die_roll, opp_die_roll, turn_count, started_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, 'unknown', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for game in games {
        stmt.execute(params![
            match_id,
            game.mtgo_game_id,
            game.game_number,
            game.result,
            game.on_play,
            game.local_mulligans,
            game.opp_mulligans,
            game.local_die_roll,
            game.opp_die_roll,
            game.turn_count,
            game.started_at_ms,
            game.ended_at_ms,
        ])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::CardCount;
    use crate::test_support::*;
    use crate::write::{MatchWrite, WriteBatch, commit};
    use rusqlite::types::Value;

    fn text(conn: &Connection, sql: &str) -> String {
        conn.query_row(sql, [], |row| row.get(0)).expect(sql)
    }

    /// Every row of `table`, every column, in id order.
    fn rows(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
        let sql = format!("SELECT * FROM {table} ORDER BY rowid");
        let mut stmt = conn.prepare(&sql).expect("prepare");
        let columns = stmt.column_count();
        let found = stmt
            .query_map([], |row| {
                let mut values = Vec::new();
                for index in 0..columns {
                    values.push(row.get::<_, Value>(index)?);
                }
                Ok(values)
            })
            .expect("query");
        found.map(|row| row.expect("row")).collect()
    }

    /// `(game_number, mtgo_game_id, result)` of every stored game.
    fn stored_games(conn: &Connection) -> Vec<(i64, Option<i64>, String)> {
        let mut stmt = conn
            .prepare("SELECT game_number, mtgo_game_id, result FROM games ORDER BY game_number, id")
            .expect("prepare");
        let found = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query");
        found.map(|row| row.expect("row")).collect()
    }

    fn import(conn: &mut Connection, matches: &[ImportedMatch]) -> ImportCounts {
        import_matches(conn, matches, 5_000).expect("import")
    }

    #[test]
    fn an_import_never_touches_a_manual_match_with_its_token() {
        let (_dir, mut conn) = db("import-manual-untouched");
        insert_manual(&conn, TOKEN);
        let matches_before = dump(&conn, "matches");
        let games_before = dump(&conn, "games");

        let counts = import(&mut conn, &[imported(TOKEN, Some(42))]);
        assert_eq!(
            counts,
            ImportCounts {
                skipped_live: 1,
                ..ImportCounts::default()
            }
        );
        assert_eq!(dump(&conn, "matches"), matches_before);
        assert_eq!(dump(&conn, "games"), games_before);
    }

    /// A 0.x manual match: game 1 with a hand after one mulligan and a
    /// revealed card, game 2 sideboarded. The 0.x match id is ignored.
    fn manual_match(token: &str) -> ImportedMatch {
        let mut found = imported(token, Some(42));
        found.games[0].local_mulligans = Some(1);
        found.manual = Some(vec![
            ManualGameDetail {
                game_number: 1,
                starting_hand: Some(vec![4001, 4001, 4002, 4004, 4004, 4004]),
                bottomed: vec![4002],
                mulligans: vec![vec![4001, 4001, 4001, 4002, 4002, 4004, 4004]],
                postboard_main: None,
                reveals: vec![CardCount {
                    catalog_id: 4003,
                    quantity: 1,
                }],
            },
            ManualGameDetail {
                game_number: 2,
                starting_hand: None,
                bottomed: Vec::new(),
                mulligans: Vec::new(),
                postboard_main: Some(vec![
                    CardCount {
                        catalog_id: 4001,
                        quantity: 4,
                    },
                    CardCount {
                        catalog_id: 4003,
                        quantity: 1,
                    },
                ]),
                reveals: Vec::new(),
            },
        ]);
        found
    }

    fn legacy_history_of(conn: &Connection, token: &str) -> (String, bool) {
        conn.query_row(
            "SELECT source, legacy_history FROM matches WHERE token = ?1",
            [token],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("match")
    }

    // Final review F5: `legacy_history` is 1 only on an `imported` or
    // `history` row (a schema CHECK). A 0.x manual match that also carries
    // 0.x's `imported` flag (hostile input) is stored with 0, whether it is
    // new or an earlier plain import becoming manual; a tracked 0.x match
    // keeps the flag.
    #[test]
    fn the_history_flag_stays_off_a_manual_match() {
        let (_dir, mut conn) = db("import-flag-manual");
        let flagged_manual = ImportedMatch {
            legacy_history: true,
            ..manual_match(TOKEN)
        };
        import(&mut conn, std::slice::from_ref(&flagged_manual));
        assert_eq!(
            legacy_history_of(&conn, TOKEN),
            ("manual".to_string(), false)
        );

        let (_dir, mut conn) = db("import-flag-becomes-manual");
        let flagged = ImportedMatch {
            legacy_history: true,
            ..imported(TOKEN, Some(42))
        };
        import(&mut conn, &[flagged]);
        assert_eq!(
            legacy_history_of(&conn, TOKEN),
            ("imported".to_string(), true)
        );
        import(&mut conn, &[flagged_manual]);
        assert_eq!(
            legacy_history_of(&conn, TOKEN),
            ("manual".to_string(), false)
        );
    }

    // Issue #332 (ruling I1): a 0.x import carrying a deck link clears the
    // link of a history copy of the same match, a deck set by hand included,
    // so 0.x's link replaces it.
    #[test]
    fn a_0x_link_clears_a_deck_set_by_hand_on_a_history_copy() {
        let (_dir, mut conn) = db("import-0x-over-hand-set-deck");
        import(&mut conn, &[imported("q-history", Some(42))]);
        conn.execute_batch(
            "UPDATE matches SET source = 'history' WHERE token = 'q-history';
             INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (5, 'q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file');
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (7, 5, 'Q', 0);",
        )
        .expect("a history copy and a deck");
        let edit = crate::match_edits::MatchEdit::SetDeckVersion {
            tokens: vec!["q-history".to_string()],
            deck_version_id: 7,
        };
        assert_eq!(
            crate::match_edits::edit_match(&mut conn, &edit).expect("edit"),
            crate::match_edits::MatchEditOutcome::Done
        );

        let linked = ImportedMatch {
            deck_linked: true,
            ..imported("q-history", Some(42))
        };
        import(&mut conn, &[linked]);
        let kept: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM matches WHERE deck_version_id = 7",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(kept, 0, "0.x's link wins over the hand-set one");
    }

    #[test]
    fn the_schema_refuses_the_history_flag_on_a_live_or_manual_row() {
        let (_dir, conn) = db("import-flag-check");
        insert_manual(&conn, TOKEN);
        for source in ["manual", "log"] {
            let refused = conn.execute(
                "UPDATE matches SET source = ?1, mtgo_match_id = NULL, legacy_history = 1",
                [source],
            );
            assert!(refused.is_err(), "{source}");
        }
        for source in ["imported", "history"] {
            conn.execute(
                "UPDATE matches SET source = ?1, legacy_history = 1",
                [source],
            )
            .expect(source);
            conn.execute("UPDATE matches SET legacy_history = 0", [])
                .expect("reset");
        }
    }

    fn details(conn: &Connection, token: &str) -> Vec<ManualGameDetail> {
        crate::manual::manual_game_rows(conn, token).expect("details")
    }

    #[test]
    fn a_manual_match_is_added_as_manual_with_its_games_and_details() {
        let (_dir, mut conn) = db("import-manual-add");
        let mut incoming = manual_match(TOKEN);
        incoming.note = Some("Q entered by hand".to_string());
        let counts = import(&mut conn, std::slice::from_ref(&incoming));
        assert_eq!(
            counts,
            ImportCounts {
                added: 1,
                ..ImportCounts::default()
            }
        );
        let one = format!("FROM matches WHERE token = '{TOKEN}'");
        assert_eq!(text(&conn, &format!("SELECT source {one}")), "manual");
        assert_eq!(
            number(
                &conn,
                &format!("SELECT COUNT(*) {one} AND mtgo_match_id IS NULL")
            ),
            1,
            "a manual match has no MTGO match id"
        );
        assert!(crate::manual::is_manual(&conn, TOKEN).expect("manual"));
        assert_eq!(
            details(&conn, TOKEN),
            incoming.manual.clone().expect("details")
        );
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(local_mulligans, ',') FROM
                     (SELECT local_mulligans FROM games ORDER BY game_number)"
            ),
            "1,0,0"
        );
        assert_eq!(
            text(&conn, "SELECT notes FROM matches"),
            "Q entered by hand"
        );
        let (view, revision) = crate::manual::manual_view(&conn, TOKEN)
            .expect("view")
            .expect("a manual view");
        assert_eq!((view.wins, view.losses, revision), (2, 1, 1));
    }

    #[test]
    fn a_manual_match_rerun_is_unchanged_and_never_overwrites_a_v1_edit() {
        let (_dir, mut conn) = db("import-manual-rerun");
        let first = manual_match(TOKEN);
        import(&mut conn, std::slice::from_ref(&first));
        let before = (
            rows(&conn, "matches"),
            rows(&conn, "games"),
            rows(&conn, "game_cards"),
            rows(&conn, "game_mulligan_cards"),
        );
        let again = import(&mut conn, std::slice::from_ref(&first));
        assert_eq!(
            again,
            ImportCounts {
                unchanged: 1,
                ..ImportCounts::default()
            }
        );
        assert_eq!(
            (
                rows(&conn, "matches"),
                rows(&conn, "games"),
                rows(&conn, "game_cards"),
                rows(&conn, "game_mulligan_cards"),
            ),
            before,
            "nothing is rewritten"
        );

        // 0.x since, no v1 edit yet: a hand for game 3 (one mulligan) fills
        // the game v1 has no record for; game 1 keeps its record.
        let mut later = first.clone();
        later.games[2].local_mulligans = Some(1);
        let mut manual = later.manual.take().expect("details");
        manual[0].reveals = vec![CardCount {
            catalog_id: 4002,
            quantity: 3,
        }];
        let game_3 = ManualGameDetail {
            game_number: 3,
            starting_hand: Some(vec![4001, 4001, 4002, 4004, 4004, 4004]),
            bottomed: vec![4004],
            mulligans: vec![Vec::new()],
            postboard_main: None,
            reveals: Vec::new(),
        };
        manual.push(game_3.clone());
        later.manual = Some(manual);
        let counts = import(&mut conn, std::slice::from_ref(&later));
        assert_eq!(
            counts,
            ImportCounts {
                updated: 1,
                ..ImportCounts::default()
            },
            "a game without a record was filled"
        );
        let mut expected = first.manual.clone().expect("details");
        expected.push(game_3);
        assert_eq!(details(&conn, TOKEN), expected);
        assert_eq!(
            text(
                &conn,
                "SELECT group_concat(local_mulligans, ',') FROM
                     (SELECT local_mulligans FROM games ORDER BY game_number)"
            ),
            "1,0,1",
            "game 3's mulligans come with its hand"
        );
        assert_eq!(number(&conn, "SELECT revision FROM matches"), 2);
        assert_eq!(import(&mut conn, std::slice::from_ref(&later)).unchanged, 1);

        // Edited in v1: game 1's reveals changed, game 2's sideboarding
        // reset to the registered deck (it now records nothing).
        let match_id = number(&conn, "SELECT id FROM matches");
        let mut v1 = details(&conn, TOKEN);
        v1[0].reveals = vec![CardCount {
            catalog_id: 4004,
            quantity: 2,
        }];
        v1[1].postboard_main = None;
        let tx = conn.transaction().expect("tx");
        for detail in &v1 {
            crate::manual::write_detail(&tx, match_id, detail).expect("v1 edit");
        }
        // Every v1 edit is a new revision (`manual::bump_revision`).
        tx.execute(
            "UPDATE matches SET revision = revision + 1 WHERE id = ?1",
            [match_id],
        )
        .expect("bump");
        tx.commit().expect("commit");
        let edited = details(&conn, TOKEN);
        assert!(edited.iter().all(|detail| detail.game_number != 2));

        // A rerun never refills or overwrites the match the player edited.
        let rerun = import(&mut conn, std::slice::from_ref(&later));
        assert_eq!(rerun.unchanged, 1);
        assert_eq!(details(&conn, TOKEN), edited, "the v1 edits are kept");
    }

    #[test]
    fn an_earlier_plain_import_of_a_manual_match_becomes_manual() {
        let (_dir, mut conn) = db("import-manual-convert");
        let manual = manual_match(TOKEN);
        let plain = ImportedMatch {
            manual: None,
            mtgo_match_id: None,
            ..manual.clone()
        };
        import(&mut conn, &[plain]);
        assert_eq!(text(&conn, "SELECT source FROM matches"), "imported");

        let counts = import(&mut conn, std::slice::from_ref(&manual));
        assert_eq!(
            counts,
            ImportCounts {
                updated: 1,
                ..ImportCounts::default()
            }
        );
        assert_eq!(text(&conn, "SELECT source FROM matches"), "manual");
        assert_eq!(
            details(&conn, TOKEN),
            manual.manual.clone().expect("details")
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 3);

        assert_eq!(import(&mut conn, &[manual]).unchanged, 1);
    }

    #[test]
    fn a_live_row_wins_over_a_manual_match_with_its_token() {
        let (_dir, mut conn) = db("import-manual-live-wins");
        live_row(&mut conn);
        let before = (rows(&conn, "matches"), rows(&conn, "games"));
        let counts = import(&mut conn, &[manual_match(TOKEN)]);
        assert_eq!(
            counts,
            ImportCounts {
                skipped_live: 1,
                ..ImportCounts::default()
            }
        );
        assert_eq!((rows(&conn, "matches"), rows(&conn, "games")), before);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM game_cards"), 0);
    }

    #[test]
    fn an_imported_match_is_added_with_its_games_accounts_and_note() {
        let (_dir, mut conn) = db("import-add");
        let mut first = imported(TOKEN, Some(42));
        first.note = Some("Q sided in the artifact hate".to_string());
        let mut second = imported(OTHER, None);
        second.opponent = None;
        second.ended_at_ms = None;
        second.state = "abandoned".to_string();
        second.games.clear();
        second.note = Some("   ".to_string());

        let counts = import(&mut conn, &[first.clone(), second]);
        assert_eq!(
            counts,
            ImportCounts {
                added: 2,
                ..ImportCounts::default()
            }
        );
        let one = format!("FROM matches WHERE token = '{TOKEN}'");
        assert_eq!(text(&conn, &format!("SELECT source {one}")), "imported");
        assert_eq!(number(&conn, &format!("SELECT mtgo_match_id {one}")), 42);
        assert_eq!(number(&conn, &format!("SELECT revision {one}")), 1);
        assert_eq!(number(&conn, &format!("SELECT derived_version {one}")), 0);
        assert_eq!(number(&conn, &format!("SELECT needs_attention {one}")), 0);
        assert_eq!(
            number(&conn, &format!("SELECT last_activity_at {one}")),
            first.ended_at_ms.expect("ended")
        );
        assert_eq!(text(&conn, &format!("SELECT outcome {one}")), "win");
        assert_eq!(text(&conn, &format!("SELECT format {one}")), "CMODERN");
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) {one} AND league_token IS NULL AND game_structure IS NULL"
                )
            ),
            1,
            "0.x matches carry no league token; their runs come with the league import"
        );
        let other = format!("FROM matches WHERE token = '{OTHER}'");
        assert_eq!(
            number(&conn, &format!("SELECT last_activity_at {other}")),
            second_started(),
            "no end time: last activity is the start"
        );
        assert_eq!(
            number(
                &conn,
                &format!(
                    "SELECT COUNT(*) {other} AND mtgo_match_id IS NULL AND opponent_id IS NULL"
                )
            ),
            1
        );

        assert_eq!(
            stored_games(&conn),
            vec![
                (1, Some(7), "win".to_string()),
                (2, Some(8), "loss".to_string()),
                (3, Some(9), "win".to_string()),
            ]
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM games WHERE end_reason = 'unknown'"
            ),
            3
        );
        assert_eq!(
            text(
                &conn,
                "SELECT concat_ws(',', on_play, local_mulligans, opp_mulligans, local_die_roll,
                     opp_die_roll, turn_count, started_at, ended_at)
                 FROM games WHERE game_number = 1"
            ),
            "1,0,1,3,5,9,1000,2000"
        );

        assert_eq!(
            number(
                &conn,
                "SELECT first_seen_at FROM accounts WHERE username = 'Qlocal'"
            ),
            5_000
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM opponents WHERE username = 'Qopponent'"
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE notes IS NOT NULL"
            ),
            1,
            "a blank note is not stored"
        );
        assert_eq!(
            text(
                &conn,
                &format!("SELECT notes FROM matches WHERE token = '{TOKEN}'")
            ),
            "Q sided in the artifact hate"
        );
    }

    /// `imported` starts every match at this time.
    fn second_started() -> i64 {
        imported(OTHER, None).started_at_ms
    }

    #[test]
    fn a_second_import_of_the_same_match_is_unchanged() {
        let (_dir, mut conn) = db("import-unchanged");
        let batch = vec![imported(TOKEN, Some(42)), imported(OTHER, None)];
        import(&mut conn, &batch);
        let before = (rows(&conn, "matches"), rows(&conn, "games"));
        let counts = import(&mut conn, &batch);
        assert_eq!(
            counts,
            ImportCounts {
                unchanged: 2,
                ..ImportCounts::default()
            }
        );
        assert_eq!(number(&conn, "SELECT MAX(revision) FROM matches"), 1);
        assert_eq!(
            (rows(&conn, "matches"), rows(&conn, "games")),
            before,
            "nothing is rewritten, not even the game rows"
        );
    }

    #[test]
    fn a_changed_match_is_updated_and_its_games_replaced() {
        let (_dir, mut conn) = db("import-update");
        let first = imported(TOKEN, Some(42));
        import(&mut conn, std::slice::from_ref(&first));
        let mut changed = first.clone();
        changed.outcome = "loss".to_string();
        changed.opponent = Some("Qrematch".to_string());
        changed.ended_at_ms = Some(first.started_at_ms + 9_000);
        changed.games = vec![
            imported_game(1, Some(7), "loss"),
            imported_game(2, Some(10), "loss"),
        ];
        let counts = import(&mut conn, &[changed.clone()]);
        assert_eq!(
            counts,
            ImportCounts {
                updated: 1,
                ..ImportCounts::default()
            }
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
        assert_eq!(number(&conn, "SELECT revision FROM matches"), 2);
        assert_eq!(text(&conn, "SELECT outcome FROM matches"), "loss");
        assert_eq!(
            number(&conn, "SELECT last_activity_at FROM matches"),
            first.started_at_ms + 9_000
        );
        assert_eq!(
            text(
                &conn,
                "SELECT o.username FROM matches m JOIN opponents o ON o.id = m.opponent_id"
            ),
            "Qrematch"
        );
        assert_eq!(
            stored_games(&conn),
            vec![
                (1, Some(7), "loss".to_string()),
                (2, Some(10), "loss".to_string())
            ]
        );

        let again = import(&mut conn, &[changed]);
        assert_eq!(again.unchanged, 1, "the updated row now matches");
        assert_eq!(number(&conn, "SELECT revision FROM matches"), 2);
    }

    #[test]
    fn a_corrected_imported_match_keeps_its_fix_through_a_re_import() {
        let (_dir, mut conn) = db("import-corrected-kept");
        let first = imported(TOKEN, Some(42));
        import(&mut conn, std::slice::from_ref(&first));
        // The player's fix: a loss in two games, new times, another opponent.
        conn.execute_batch(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qfixed', 1);
             UPDATE matches SET outcome = 'loss', started_at = 500, ended_at = 700,
                 opponent_id = (SELECT id FROM opponents WHERE username = 'Qfixed'),
                 corrected_at = 800, revision = revision + 1;
             UPDATE games SET result = 'loss';
             DELETE FROM games WHERE game_number = 3;",
        )
        .expect("the fix");
        let matches_before = rows(&conn, "matches");
        let games_before = rows(&conn, "games");

        let mut changed = first.clone();
        changed.outcome = "draw".to_string();
        changed.opponent = Some("Qrematch".to_string());
        changed.ended_at_ms = Some(first.started_at_ms + 9_000);
        let counts = import(&mut conn, &[first, changed]);
        assert_eq!(
            counts,
            ImportCounts {
                unchanged: 2,
                ..ImportCounts::default()
            }
        );
        assert_eq!(rows(&conn, "matches"), matches_before, "the row is kept");
        assert_eq!(rows(&conn, "games"), games_before, "its games are kept");
    }

    #[test]
    fn a_0x_import_takes_the_fix_of_a_corrected_history_copy_it_replaces() {
        let (_dir, mut conn) = db("import-takes-corrected-history");
        import(&mut conn, &[imported("q-history", Some(42))]);
        conn.execute_batch(
            "UPDATE matches SET source = 'history';
             INSERT INTO opponents (username, first_seen_at) VALUES ('Qfixed', 1);
             UPDATE matches SET outcome = 'loss', started_at = 500, ended_at = 700,
                 opponent_id = (SELECT id FROM opponents WHERE username = 'Qfixed'),
                 corrected_at = 800;
             UPDATE games SET result = 'loss';
             DELETE FROM games WHERE game_number = 3;",
        )
        .expect("the fixed history copy");

        let zero_x = imported(TOKEN, Some(42));
        import(&mut conn, std::slice::from_ref(&zero_x));
        assert_eq!(
            text(
                &conn,
                "SELECT m.token || '|' || m.source || '|' || m.outcome || '|' || m.started_at
                     || '|' || m.ended_at || '|' || o.username || '|' || m.corrected_at
                 FROM matches m JOIN opponents o ON o.id = m.opponent_id"
            ),
            format!("{TOKEN}|imported|loss|500|700|Qfixed|800"),
            "the 0.x row carries the fix and the copy goes"
        );
        assert_eq!(
            text(&conn, "SELECT group_concat(result, ',') FROM games"),
            "loss,loss"
        );

        // A rerun leaves the fixed row alone.
        let counts = import(&mut conn, &[zero_x]);
        assert_eq!(counts.unchanged, 1);
        assert_eq!(
            text(&conn, "SELECT group_concat(result, ',') FROM games"),
            "loss,loss"
        );
    }

    #[test]
    fn a_live_row_is_never_touched_by_token_or_by_match_id() {
        let (_dir, mut conn) = db("import-live-wins");
        live_row(&mut conn);
        let before: Vec<Vec<Vec<Value>>> = ["matches", "games", "accounts", "opponents"]
            .iter()
            .map(|table| rows(&conn, table))
            .collect();

        let mut same_token = imported(TOKEN, Some(5));
        same_token.local_username = "Qelsewhere".to_string();
        same_token.opponent = Some("Qimported".to_string());
        same_token.note = Some("Q note by token".to_string());
        let mut same_match_id = imported(OTHER, Some(99));
        same_match_id.local_username = "Qelsewhere".to_string();
        same_match_id.opponent = Some("Qimported".to_string());
        same_match_id.note = Some("Q note by match id".to_string());

        let counts = import(&mut conn, &[same_token, same_match_id]);
        assert_eq!(
            counts,
            ImportCounts {
                skipped_live: 2,
                ..ImportCounts::default()
            }
        );
        // The 0.x note fills the live row's empty note (an input column, see
        // the note tests below); every other column stays as it was.
        assert_eq!(text(&conn, "SELECT notes FROM matches"), "Q note by token");
        conn.execute("UPDATE matches SET notes = NULL", [])
            .expect("set the note aside");
        let after: Vec<Vec<Vec<Value>>> = ["matches", "games", "accounts", "opponents"]
            .iter()
            .map(|table| rows(&conn, table))
            .collect();
        assert_eq!(after, before, "the live row, its games and the registries");
    }

    /// The live match: token TOKEN, Match ID 99.
    fn live_row(conn: &mut Connection) {
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 4,
                }],
                now_ms: 1,
                ..WriteBatch::default()
            },
        )
        .expect("live row");
    }

    /// Every stored note as `token: body`.
    fn notes(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT token || ': ' || notes FROM matches WHERE notes IS NOT NULL ORDER BY token",
            )
            .expect("prepare");
        let found = stmt.query_map([], |row| row.get(0)).expect("query");
        found.map(|row| row.expect("row")).collect()
    }

    #[test]
    fn a_match_skipped_by_token_keeps_its_note_on_the_live_token() {
        let (_dir, mut conn) = db("import-live-note-token");
        live_row(&mut conn);
        let mut same_token = imported(TOKEN, Some(5));
        same_token.note = Some("Q note by token".to_string());
        let counts = import(&mut conn, &[same_token]);
        assert_eq!(counts.skipped_live, 1);
        assert_eq!(notes(&conn), vec![format!("{TOKEN}: Q note by token")]);
    }

    #[test]
    fn a_match_skipped_by_match_id_keeps_its_note_on_the_live_token() {
        let (_dir, mut conn) = db("import-live-note-match-id");
        live_row(&mut conn);
        let mut blank = imported(OTHER, Some(99));
        blank.note = Some("   ".to_string());
        assert_eq!(import(&mut conn, &[blank]).skipped_live, 1);
        assert!(notes(&conn).is_empty(), "a blank note is not stored");

        let mut same_match_id = imported(OTHER, Some(99));
        same_match_id.note = Some("Q note by match id".to_string());
        let counts = import(&mut conn, &[same_match_id]);
        assert_eq!(counts.skipped_live, 1);
        assert_eq!(
            notes(&conn),
            vec![format!("{TOKEN}: Q note by match id")],
            "keyed by the live row's token, not the 0.x token"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM matches"), 1);
    }

    #[test]
    fn a_skipped_live_match_never_overwrites_the_live_note() {
        let (_dir, mut conn) = db("import-live-note-kept");
        live_row(&mut conn);
        conn.execute(
            "UPDATE matches SET notes = 'Q written in v1' WHERE token = ?1",
            [TOKEN],
        )
        .expect("v1 note");
        let mut same_token = imported(TOKEN, Some(5));
        same_token.note = Some("Q from 0.x by token".to_string());
        let mut same_match_id = imported(OTHER, Some(99));
        same_match_id.note = Some("Q from 0.x by match id".to_string());
        let counts = import(&mut conn, &[same_token, same_match_id]);
        assert_eq!(counts.skipped_live, 2);
        assert_eq!(
            text(&conn, "SELECT notes FROM matches WHERE notes IS NOT NULL"),
            "Q written in v1"
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE notes IS NOT NULL"
            ),
            1
        );
    }

    #[test]
    fn a_note_already_in_v1_is_never_overwritten() {
        let (_dir, mut conn) = db("import-note-kept");
        // The match imported without a note, then noted in v1.
        import(&mut conn, &[imported(TOKEN, Some(42))]);
        conn.execute(
            "UPDATE matches SET notes = 'Q written in v1' WHERE token = ?1",
            [TOKEN],
        )
        .expect("v1 note");
        let mut by_token = imported(TOKEN, Some(42));
        by_token.note = Some("Q from 0.x".to_string());
        let mut later = imported(OTHER, Some(43));
        later.note = Some("Q first import".to_string());
        import(&mut conn, &[by_token, later.clone()]);

        later.note = Some("Q second import".to_string());
        import(&mut conn, &[later]);

        assert_eq!(
            text(
                &conn,
                &format!("SELECT notes FROM matches WHERE token = '{TOKEN}'")
            ),
            "Q written in v1"
        );
        assert_eq!(
            text(
                &conn,
                &format!("SELECT notes FROM matches WHERE token = '{OTHER}'")
            ),
            "Q first import"
        );
    }

    #[test]
    fn two_games_with_the_same_game_id_keep_the_first() {
        let (_dir, mut conn) = db("import-duplicate-game");
        let mut clashing = imported(TOKEN, Some(42));
        clashing.games = vec![
            imported_game(1, Some(7), "win"),
            imported_game(2, Some(7), "loss"),
            imported_game(3, None, "win"),
            imported_game(4, None, "draw"),
        ];
        let counts = import(&mut conn, std::slice::from_ref(&clashing));
        assert_eq!(counts.added, 1);
        assert_eq!(
            stored_games(&conn),
            vec![
                (1, Some(7), "win".to_string()),
                (3, None, "win".to_string()),
                (4, None, "draw".to_string()),
            ],
            "the second Game ID 7 is dropped; games without an ID are all kept"
        );
        let again = import(&mut conn, &[clashing]);
        assert_eq!(
            again.unchanged, 1,
            "a re-run compares against the kept games"
        );
    }

    /// A 0.x match without game rows, scored 1-2 in 0.x.
    fn gameless(token: &str, mtgo_match_id: Option<i64>) -> ImportedMatch {
        let mut found = imported(token, mtgo_match_id);
        found.games.clear();
        found.outcome = "loss".to_string();
        found.games_won_imported = Some(1);
        found.games_lost_imported = Some(2);
        found
    }

    /// `games_won_imported,games_lost_imported` of the row with `token`
    /// (empty for NULL).
    fn imported_score(conn: &Connection, token: &str) -> String {
        text(
            conn,
            &format!(
                "SELECT concat_ws(',', IFNULL(games_won_imported, ''),
                     IFNULL(games_lost_imported, ''))
                 FROM matches WHERE token = '{token}'"
            ),
        )
    }

    #[test]
    fn a_gameless_match_keeps_its_0x_score_and_a_rerun_writes_nothing() {
        let (_dir, mut conn) = db("import-gameless-score");
        let batch = vec![gameless(TOKEN, Some(42)), imported(OTHER, Some(43))];
        assert_eq!(import(&mut conn, &batch).added, 2);
        assert_eq!(imported_score(&conn, TOKEN), "1,2");
        assert_eq!(
            imported_score(&conn, OTHER),
            ",",
            "a match with games keeps no score of its own"
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM games"), 3);

        let before = (rows(&conn, "matches"), rows(&conn, "games"));
        let again = import(&mut conn, &batch);
        assert_eq!(
            again,
            ImportCounts {
                unchanged: 2,
                ..ImportCounts::default()
            }
        );
        assert_eq!((rows(&conn, "matches"), rows(&conn, "games")), before);

        // 0.x's score changed since: the rerun keeps it in step.
        let mut changed = gameless(TOKEN, Some(42));
        changed.games_won_imported = Some(2);
        assert_eq!(import(&mut conn, &[changed]).updated, 1);
        assert_eq!(imported_score(&conn, TOKEN), "2,2");
    }

    #[test]
    fn a_live_row_replacing_a_gameless_0x_match_drops_its_score() {
        let (_dir, mut conn) = db("import-gameless-live");
        import(&mut conn, &[gameless(TOKEN, Some(42))]);
        live_row(&mut conn);
        assert_eq!(text(&conn, "SELECT source FROM matches"), "log");
        assert_eq!(imported_score(&conn, TOKEN), ",");
        let counts = import(&mut conn, &[gameless(TOKEN, Some(42))]);
        assert_eq!(counts.skipped_live, 1, "live wins");
        assert_eq!(imported_score(&conn, TOKEN), ",");
    }

    fn account(
        username: &str,
        login_id: Option<i64>,
        created_at_ms: Option<i64>,
    ) -> ImportedAccount {
        ImportedAccount {
            username: username.to_string(),
            login_id,
            created_at_ms,
            tracked: true,
            active_since_ms: None,
        }
    }

    /// Every v1 account as `username,tracked`, by username.
    fn tracking(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT concat_ws(',', username, tracked) FROM accounts ORDER BY username")
            .expect("prepare");
        let found = stmt.query_map([], |row| row.get(0)).expect("query");
        found.map(|row| row.expect("row")).collect()
    }

    fn untracked(username: &str) -> ImportedAccount {
        ImportedAccount {
            tracked: false,
            ..account(username, None, Some(1))
        }
    }

    fn set_tracked(conn: &mut Connection, username: &str, tracked: bool) {
        let edit = crate::accounts::AccountEdit::SetTracked {
            username: username.to_string(),
            tracked,
        };
        crate::accounts::edit_account(conn, &edit).expect("set tracked");
    }

    fn account_id_of(conn: &Connection, username: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [username],
            |row| row.get(0),
        )
        .expect(username)
    }

    // Issue #310: an account untracked in 0.x arrives untracked; a choice
    // made in v1 (before the import or since) is never overwritten.
    #[test]
    fn an_account_untracked_in_0x_arrives_untracked_unless_v1_chose() {
        let (_dir, mut conn) = db("import-accounts-tracked");
        conn.execute_batch(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qseen', 1), ('Qchosen', 1);",
        )
        .expect("v1 accounts");
        // The user turned Qchosen off and on again in v1: tracked, by choice.
        set_tracked(&mut conn, "Qchosen", false);
        set_tracked(&mut conn, "Qchosen", true);
        let batch = vec![
            untracked("Qnew"),
            untracked("Qseen"),
            untracked("Qchosen"),
            account("Qkept", None, Some(1)),
        ];
        let counts = import_accounts_at(&mut conn, &batch);
        assert_eq!((counts.added, counts.untracked), (2, 2));
        assert_eq!(
            tracking(&conn),
            vec!["Qchosen,1", "Qkept,1", "Qnew,0", "Qseen,0"],
            "the v1 choice wins; an account v1 only saw takes 0.x's flag"
        );

        // Tracked again in v1 after the import: a rerun leaves it.
        set_tracked(&mut conn, "Qnew", true);
        let again = import_accounts_at(&mut conn, &batch);
        assert_eq!(again.untracked, 0);
        assert_eq!(
            tracking(&conn),
            vec!["Qchosen,1", "Qkept,1", "Qnew,1", "Qseen,0"]
        );
    }

    // Issue #310 (and #316 row 7): 0.x's active account seeds v1's switch
    // when v1 has none, at the time 0.x last made it active.
    #[test]
    fn the_0x_active_account_seeds_the_switch_when_v1_has_none() {
        let (_dir, mut conn) = db("import-accounts-active");
        // Settings saved before any switch store a `null`: no switch.
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('active_account', 'null')",
            [],
        )
        .expect("a saved null");
        let batch = vec![
            account("Qone", None, Some(1)),
            ImportedAccount {
                active_since_ms: Some(3_000),
                ..account("Qtwo", None, Some(1))
            },
        ];
        let counts = import_accounts_at(&mut conn, &batch);
        assert!(counts.active_seeded);
        let two = account_id_of(&conn, "Qtwo");
        assert_eq!(
            crate::accounts::active_account(&conn).expect("read"),
            Some(crate::accounts::ActiveAccount {
                account_id: two,
                switched_at_ms: 3_000,
            })
        );

        // The user switches in v1: a rerun leaves the switch.
        let edit = crate::accounts::AccountEdit::Switch {
            username: "Qone".to_string(),
            at_ms: 9_000,
        };
        crate::accounts::edit_account(&mut conn, &edit).expect("switch");
        let again = import_accounts_at(&mut conn, &batch);
        assert!(!again.active_seeded);
        assert_eq!(
            crate::accounts::active_account(&conn).expect("read"),
            Some(crate::accounts::ActiveAccount {
                account_id: account_id_of(&conn, "Qone"),
                switched_at_ms: 9_000,
            })
        );
    }

    fn active(username: &str, since_ms: i64) -> ImportedAccount {
        ImportedAccount {
            active_since_ms: Some(since_ms),
            ..account(username, None, Some(1))
        }
    }

    fn seeded_account(conn: &Connection) -> Option<i64> {
        crate::accounts::active_account(conn)
            .expect("read")
            .map(|switch| switch.account_id)
    }

    // Review M3: 0.x's `saved` hook can leave two active rows. The one made
    // active last wins; on a tie, the first in 0.x's id order.
    #[test]
    fn with_several_active_0x_accounts_the_newest_seeds_the_switch() {
        let (_dir, mut conn) = db("import-accounts-active-several");
        let counts = import_accounts_at(
            &mut conn,
            &[active("Qolder", 1_000), active("Qnewer", 2_000)],
        );
        assert!(counts.active_seeded);
        assert_eq!(seeded_account(&conn), Some(account_id_of(&conn, "Qnewer")));

        let (_dir2, mut tie) = db("import-accounts-active-tie");
        import_accounts_at(
            &mut tie,
            &[active("Qfirst", 1_000), active("Qsecond", 1_000)],
        );
        assert_eq!(seeded_account(&tie), Some(account_id_of(&tie, "Qfirst")));
    }

    #[test]
    fn a_switch_made_in_v1_before_the_import_wins() {
        let (_dir, mut conn) = db("import-accounts-active-v1");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qone', 1)",
            [],
        )
        .expect("v1 account");
        let edit = crate::accounts::AccountEdit::Switch {
            username: "Qone".to_string(),
            at_ms: 2,
        };
        crate::accounts::edit_account(&mut conn, &edit).expect("switch");
        let counts = import_accounts_at(
            &mut conn,
            &[ImportedAccount {
                active_since_ms: Some(3_000),
                ..account("Qtwo", None, Some(1))
            }],
        );
        assert!(!counts.active_seeded);
        assert_eq!(
            crate::accounts::active_account(&conn).expect("read"),
            Some(crate::accounts::ActiveAccount {
                account_id: account_id_of(&conn, "Qone"),
                switched_at_ms: 2,
            })
        );
    }

    /// Every v1 account as `username,login_id,first_seen_at`, by username.
    fn accounts(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT concat_ws(',', username, IFNULL(login_id, ''), first_seen_at)
                 FROM accounts ORDER BY username",
            )
            .expect("prepare");
        let found = stmt.query_map([], |row| row.get(0)).expect("query");
        found.map(|row| row.expect("row")).collect()
    }

    fn import_accounts_at(conn: &mut Connection, batch: &[ImportedAccount]) -> AccountImportCounts {
        import_accounts(conn, batch, 5_000).expect("accounts")
    }

    #[test]
    fn an_account_without_matches_is_registered_from_its_0x_creation_time() {
        let (_dir, mut conn) = db("import-accounts-new");
        let counts = import_accounts_at(
            &mut conn,
            &[
                account("Qquiet", Some(7), Some(1_000)),
                account("Qundated", None, None),
            ],
        );
        assert_eq!(
            counts,
            AccountImportCounts {
                added: 2,
                login_ids_filled: 1,
                ..AccountImportCounts::default()
            }
        );
        assert_eq!(
            accounts(&conn),
            vec!["Qquiet,7,1000", "Qundated,,5000"],
            "no creation time: the import time"
        );
    }

    #[test]
    fn a_login_id_fills_a_null_and_never_replaces_one() {
        let (_dir, mut conn) = db("import-accounts-login");
        conn.execute_batch(
            "INSERT INTO accounts (username, login_id, first_seen_at)
             VALUES ('Qlocal', NULL, 10), ('Qtracked', 5, 20);",
        )
        .expect("v1 accounts");
        let batch = vec![
            account("Qlocal", Some(7), Some(1)),
            account("Qtracked", Some(9), Some(1)),
        ];
        let counts = import_accounts_at(&mut conn, &batch);
        assert_eq!(
            counts,
            AccountImportCounts {
                unchanged: 2,
                login_ids_filled: 1,
                ..AccountImportCounts::default()
            }
        );
        assert_eq!(
            accounts(&conn),
            vec!["Qlocal,7,1", "Qtracked,5,1"],
            "v1's login id is kept; 0.x's earlier creation time lowers first seen (#190)"
        );

        let before = rows(&conn, "accounts");
        let again = import_accounts_at(&mut conn, &batch);
        assert_eq!(
            again,
            AccountImportCounts {
                unchanged: 2,
                ..AccountImportCounts::default()
            }
        );
        assert_eq!(rows(&conn, "accounts"), before, "a rerun writes nothing");
    }

    #[test]
    fn a_login_id_another_account_has_is_a_counted_clash() {
        let (_dir, mut conn) = db("import-accounts-clash");
        conn.execute(
            "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qtracked', 5, 20)",
            [],
        )
        .expect("v1 account");
        let batch = vec![account("Qrenamed", Some(5), Some(1))];
        let counts = import_accounts_at(&mut conn, &batch);
        assert_eq!(
            counts,
            AccountImportCounts {
                added: 1,
                login_id_clashes: 1,
                ..AccountImportCounts::default()
            }
        );
        assert_eq!(accounts(&conn), vec!["Qrenamed,,1", "Qtracked,5,20"]);
        let again = import_accounts_at(&mut conn, &batch);
        assert_eq!(
            again,
            AccountImportCounts {
                unchanged: 1,
                ..AccountImportCounts::default()
            },
            "a rerun does not count the same clash again (#190)"
        );
    }

    #[test]
    fn a_later_0x_creation_time_never_raises_first_seen() {
        let (_dir, mut conn) = db("import-accounts-first-seen");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 10)",
            [],
        )
        .expect("v1 account");
        import_accounts_at(
            &mut conn,
            &[
                account("Qlocal", None, Some(50)),
                account("Qlocal", None, None),
            ],
        );
        assert_eq!(accounts(&conn), vec!["Qlocal,,10"]);
    }
}
