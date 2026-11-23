//! One population run (cards spec section 6, 0.x `PopulateMissingCardData`):
//!
//! 1. nameless stubs are matched against MTGO's token XML (the first
//!    `CardDataSource` folder under the MTGO data folder);
//! 2. every row without a Scryfall id (the token names laid over) goes
//!    through Q6's id, back face and by-name passes (`look_up_cards`, class
//!    `Reference`: sent offline and before sign-in);
//! 3. what was learned goes to the tracker as one `CardsResolved`;
//! 4. until it has run once with the API sending costs, the resolved rows
//!    stored without a mana cost (from before the API sent costs) are looked
//!    up again (`refresh_mana_costs`).
//!
//! A failed request answers nothing (Q6), so its rows stay unresolved for
//! the next run. Blocking: runs on the cards worker thread.

use std::fs;
use std::path::{Path, PathBuf};

use cards::tokens::{TokenCard, parse_string_table, parse_token_file, token_cards};
use mtgo_log::deck_files::{DECK_MAX_DEPTH, deck_roots};
use mtgo_log::discover::Env;
use mymtgo_api::{Resolutions, UnresolvedCard};
use store::Connection;
use store::cards::{ResolveOutcome, UNKNOWN_RECHECK_MS};

use crate::api::reference::{CardsLookup, look_up_cards, look_up_mana_costs};
use crate::api::session::ApiState;

/// The folder MTGO keeps its card data XML in.
pub const CARD_DATA_SOURCE: &str = "CardDataSource";

/// The app version: a new one asks about every unknown card again.
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The token file in it.
pub const TOKEN_FILE: &str = "client_TOK.xml";

/// The first directory named `CardDataSource` under `root`, depth-first in
/// name order (0.x's Finder). Unreadable directories are skipped and linked
/// directories are not entered.
pub fn find_card_data_source(root: &Path) -> Option<PathBuf> {
    search(root, 0)
}

fn search(dir: &Path, depth: usize) -> Option<PathBuf> {
    if depth >= DECK_MAX_DEPTH {
        return None;
    }
    let entries = fs::read_dir(dir).ok()?;
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    for folder in folders {
        if folder
            .file_name()
            .is_some_and(|name| name == CARD_DATA_SOURCE)
        {
            return Some(folder);
        }
        if let Some(found) = search(&folder, depth + 1) {
            return Some(found);
        }
    }
    None
}

/// The `CardDataSource` folder under the chosen MTGO data folder, else under
/// the detected ones.
pub fn card_data_source(env: &Env, data_folder: Option<&Path>) -> Option<PathBuf> {
    deck_roots(env, data_folder)
        .iter()
        .find_map(|root| find_card_data_source(root))
}

/// The tokens for these nameless stubs from the folder's XML files. A
/// missing folder or token file gives none; a missing string table is
/// empty (no names: no tokens).
pub fn read_tokens(folder: Option<&Path>, nameless: &[u32]) -> Vec<TokenCard> {
    let Some(folder) = folder else {
        return Vec::new();
    };
    let Ok(token_xml) = fs::read_to_string(folder.join(TOKEN_FILE)) else {
        return Vec::new();
    };
    let table = |name: &str| {
        let text = fs::read_to_string(folder.join(format!("{name}.xml"))).unwrap_or_default();
        parse_string_table(&text, name)
    };
    token_cards(
        &parse_token_file(&token_xml),
        &table("CARDNAME_STRING"),
        &table("COLOR"),
        &table("CREATURE_TYPE_STRING0"),
        nameless,
    )
}

/// Lay the token names over the unresolved rows, as 0.x's XML pass stored
/// them before the API passes read the rows: a nameless row a token names
/// is asked about as a token.
pub fn with_token_names(
    mut cards: Vec<UnresolvedCard>,
    tokens: &[TokenCard],
) -> Vec<UnresolvedCard> {
    for card in &mut cards {
        if card.name.is_some() {
            continue;
        }
        if let Some(token) = tokens.iter().find(|token| token.catalog_id == card.mtgo_id) {
            card.name = Some(token.name.clone());
            card.is_token = true;
        }
    }
    cards
}

/// What a run needs, so tests can run it without the app.
pub struct Population<'a> {
    pub api: &'a ApiState,
    /// A fresh read-only connection, `None` when it cannot be opened.
    pub reader: &'a dyn Fn() -> Option<Connection>,
    /// The `CardDataSource` folder, `None` when there is none.
    pub card_data: &'a dyn Fn() -> Option<PathBuf>,
    /// Sends `CardsResolved` to the tracker.
    /// Sends `CardsResolved` to the tracker; the ids the API answered
    /// without are marked unknown at the given time.
    pub resolve: &'a dyn Fn(Vec<TokenCard>, Resolutions, i64) -> Result<ResolveOutcome, String>,
    /// Stores settings rows through the tracker (the refresh's marker).
    pub save_settings: &'a dyn Fn(Vec<(String, String)>) -> Result<(), String>,
    pub now: &'a dyn Fn() -> i64,
    /// A button asked for this run ("Download card data", "Fetch
    /// missing"): the ids the API did not know are asked about again.
    pub ask_unknown: bool,
}

/// Run population once. Errors are what the "Download card data" button
/// shows after "Card population failed: ".
pub fn populate(run: &Population) -> Result<(), String> {
    let conn = (run.reader)().ok_or_else(|| "could not open the database".to_string())?;
    let nameless = store::cards::nameless(&conn).map_err(|error| error.to_string())?;
    let tokens = if nameless.is_empty() {
        Vec::new()
    } else {
        read_tokens((run.card_data)().as_deref(), &nameless)
    };
    // An id the API answered without is passed over for a while (#501),
    // but a new app version, or a button, asks about every one once more.
    let checked =
        store::cards::unknown_checked_version(&conn).map_err(|error| error.to_string())?;
    let version_changed = checked.as_deref() != Some(APP_VERSION);
    let unknown_since =
        (!version_changed && !run.ask_unknown).then(|| (run.now)() - UNKNOWN_RECHECK_MS);
    let unresolved =
        store::cards::unresolved(&conn, unknown_since).map_err(|error| error.to_string())?;
    let refresh = if store::cards::mana_costs_filled(&conn).map_err(|error| error.to_string())? {
        None
    } else {
        Some(CostRefresh {
            cards: store::cards::mana_cost_refresh(&conn).map_err(|error| error.to_string())?,
            attempts: store::cards::mana_cost_attempts(&conn).map_err(|error| error.to_string())?,
        })
    };
    drop(conn);
    let cards = with_token_names(unresolved, &tokens);
    let lookup = if cards.is_empty() {
        CardsLookup::default()
    } else {
        look_up_cards(run.api, &cards, (run.now)())
    };
    let resolutions = lookup.resolutions;
    if !tokens.is_empty() || !resolutions.resolved.is_empty() || !resolutions.unknown.is_empty() {
        let outcome = (run.resolve)(tokens, resolutions, (run.now)())?;
        tracing::info!(
            rows = outcome.updated,
            changed = outcome.changed.len(),
            "card data stored"
        );
    }
    // Every unresolved id was asked and answered: the new version's recheck
    // is done. After a failed request it waits for a run that is answered.
    if version_changed && !lookup.failed {
        (run.save_settings)(vec![store::cards::unknown_checked_version_row(APP_VERSION)])?;
    }
    match refresh {
        Some(refresh) => refresh_mana_costs(run, &refresh),
        None => Ok(()),
    }
}

/// The rows the mana cost refresh asks about, in catalog id order, and the
/// failed refresh runs so far.
struct CostRefresh {
    cards: Vec<UnresolvedCard>,
    attempts: u32,
}

/// The refresh rows asked and stored together, so a run cut short keeps
/// what it stored and no one store is large.
const REFRESH_BATCH: usize = 200;

/// Look the rows stored without a mana cost up again, a batch at a time,
/// and store what comes back. The refresh is recorded as done
/// (`MANA_COSTS_FILLED`) when there is nothing left to ask, when every
/// request answered with costs, or after `MANA_COST_ATTEMPTS_MAX` runs with
/// a failed request once costs were seen (in that run or an earlier failed
/// one, as the rows still asked may be only the failing ones). Until then each run asks again, but a
/// row that got its cost is not asked twice and an API without costs is
/// asked once a run. After that, a card the API holds no cost for (a modal
/// or transforming card) is never asked about again.
fn refresh_mana_costs(run: &Population, refresh: &CostRefresh) -> Result<(), String> {
    let done = vec![store::cards::mana_costs_filled_row()];
    if refresh.cards.is_empty() {
        return (run.save_settings)(done);
    }
    let mut costs_sent = None;
    let mut failed = false;
    for batch in refresh.cards.chunks(REFRESH_BATCH) {
        let lookup = look_up_mana_costs(run.api, batch, costs_sent, (run.now)());
        costs_sent = lookup.costs_sent;
        failed = lookup.failed;
        if costs_sent == Some(false) {
            return Ok(());
        }
        if lookup.failed && costs_sent.is_none() && refresh.attempts == 0 {
            // Failed before any answer, and costs never seen: the API may
            // be down or throttled, so this run does not count.
            return Ok(());
        }
        if !lookup.resolutions.resolved.is_empty() {
            let answered = Resolutions {
                resolved: lookup.resolutions.resolved,
                // The rows are resolved already: none is unknown.
                unknown: Vec::new(),
            };
            let outcome = (run.resolve)(Vec::new(), answered, (run.now)())?;
            tracing::info!(rows = outcome.updated, "card mana costs stored");
        }
        if failed {
            break;
        }
    }
    if !failed {
        return (run.save_settings)(done);
    }
    let attempts = refresh.attempts + 1;
    if attempts >= store::cards::MANA_COST_ATTEMPTS_MAX {
        tracing::warn!(
            attempts,
            "the mana cost refresh gave up after failed requests"
        );
        return (run.save_settings)(done);
    }
    (run.save_settings)(vec![store::cards::mana_cost_attempts_row(attempts)])
}

#[cfg(test)]
pub(crate) mod tests;
