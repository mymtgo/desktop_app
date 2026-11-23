//! The deck a platform match was played with (Arena, Endstep): its deck
//! file sync and the link of the match to the version it creates, shared by
//! every source whose matches name their deck list (MTGO's own deck sync is
//! `deck_sync`).
//!
//! The deck write commits first; the match is linked in a later batch, by
//! exact signature among the account's versions (Arena ruling 3). A deck
//! used again while an earlier match of it still waits for its link is held
//! back: its write could prune the version the earlier match needs.

use decks::{CardInfo, DeckCard, DeckFile, MAX_CARD_QUANTITY};
use store::decks::{
    DeckWrite, IdentityUpdate, account_versions, card_infos, deck_by_mtgo_id, link_of, versions_of,
};
use store::write::WriteBatch;
use store::{Connection, Platform};

use crate::TrackerError;

/// A match to link to its deck version once the deck write that creates the
/// version has committed (Arena ruling 3).
#[derive(Debug, Clone)]
pub(crate) struct PendingLink {
    pub token: String,
    pub account: String,
    pub deck_id: String,
    pub signature: String,
    /// A throwaway deck (`decks.mtgo_id`) the match may have used before
    /// this link: archived after it when left unused and untouched (#867,
    /// `DeckWrite::ArchiveUnused`). Endstep's match-keyed deck; always
    /// `None` for Arena.
    pub leftover: Option<String>,
}

/// The deck a match used, with its cards as catalog ids and counts.
#[derive(Debug, Clone)]
pub(crate) struct DeckUseIds {
    pub match_token: String,
    /// The deck's key (`decks.mtgo_id`): Arena's deck id, `endstep:<id>`.
    pub deck_id: String,
    pub deck_name: String,
    pub format: String,
    pub main: Vec<(u32, u32)>,
    pub side: Vec<(u32, u32)>,
}

/// The deck file sync of the deck a match used (as `account`, at `at_ms`),
/// and the link that waits for it. Endstep's matches can be folded out of
/// play order, so its list always becomes a version, even when older than
/// the deck's last write (`DeckWrite::SyncUse`); Arena reads its log in
/// order and syncs like a deck file (`DeckWrite::SyncFile`).
pub(crate) fn deck_sync_write(
    reader: &Connection,
    deck: DeckUseIds,
    account: &str,
    at_ms: i64,
    now_ms: i64,
    platform: Platform,
) -> Result<(DeckWrite, PendingLink), TrackerError> {
    let mut cards = deck_cards(&deck.main, false);
    cards.extend(deck_cards(&deck.side, true));
    let signature = decks::signature(&cards);
    let identity = identity_update(reader, &cards)?;
    let link = PendingLink {
        token: deck.match_token,
        account: account.to_string(),
        deck_id: deck.deck_id.clone(),
        signature: signature.clone(),
        leftover: None,
    };
    let file = DeckFile {
        name: deck.deck_name,
        net_deck_id: deck.deck_id,
        format: deck.format,
        timestamp_ms: at_ms,
        cards,
    };
    let account = Some(account.to_string());
    let write = if platform == Platform::Endstep {
        DeckWrite::SyncUse {
            file,
            signature,
            account,
            platform,
            synced_at_ms: now_ms,
            identity,
        }
    } else {
        DeckWrite::SyncFile {
            file,
            signature,
            account,
            platform,
            synced_at_ms: now_ms,
            identity,
        }
    };
    Ok((write, link))
}

/// Whether the match is linked already to a version of the link's deck with
/// the link's signature: the deck write and the link would change nothing.
pub(crate) fn is_linked_as(reader: &Connection, link: &PendingLink) -> Result<bool, TrackerError> {
    let Some(version_id) = link_of(reader, &link.token)? else {
        return Ok(false);
    };
    let Some(deck) = deck_by_mtgo_id(reader, &link.deck_id)? else {
        return Ok(false);
    };
    for version in versions_of(reader, deck.id)? {
        if version.id == version_id && version.signature == link.signature {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The deck writes for the next batch and the matches waiting to be linked
/// once they have committed.
#[derive(Debug, Default)]
pub(crate) struct DeckLinks {
    /// Deck writes for the next batch, in order.
    pub deck_writes: Vec<DeckWrite>,
    /// Matches to link once their deck writes have committed.
    pub links: Vec<PendingLink>,
    /// Deck uses whose deck still has a link waiting: the deck write waits
    /// too, since it could prune the version the earlier match needs.
    pub held: Vec<(DeckWrite, PendingLink)>,
    /// The deck ids (`decks.mtgo_id`) whose sync went into a batch since the
    /// engine last took them: it asks the archetype prefill about the ones
    /// still without an archetype, as for MTGO's deck files (#690).
    written: Vec<String>,
}

impl DeckLinks {
    /// Queue a deck write and its link, or hold both while the same deck
    /// still has a link waiting.
    pub fn push(&mut self, write: DeckWrite, link: PendingLink) {
        let waiting = self.links.iter().any(|other| other.deck_id == link.deck_id)
            || self
                .held
                .iter()
                .any(|(_, other)| other.deck_id == link.deck_id);
        if waiting {
            self.held.push((write, link));
        } else {
            self.deck_writes.push(write);
            self.links.push(link);
        }
    }

    /// An account renamed (#862): the queued deck writes and the waiting
    /// links under its old name take the new one. The batch writes the
    /// rename before any deck, so the old name is gone by then.
    pub fn rename_account(&mut self, from: &str, to: &str) {
        for write in &mut self.deck_writes {
            rename_in_write(write, from, to);
        }
        for link in &mut self.links {
            rename_in_link(link, from, to);
        }
        for (write, link) in &mut self.held {
            rename_in_write(write, from, to);
            rename_in_link(link, from, to);
        }
    }

    /// Move the queued deck writes into `batch`, noting each synced deck.
    pub fn write_into(&mut self, batch: &mut WriteBatch) {
        for write in &self.deck_writes {
            if let DeckWrite::SyncFile { file, .. } | DeckWrite::SyncUse { file, .. } = write {
                self.written.push(file.net_deck_id.clone());
            }
        }
        batch.decks.append(&mut self.deck_writes);
    }

    /// The deck ids synced since the last call (see `written`).
    pub fn take_written(&mut self) -> Vec<String> {
        std::mem::take(&mut self.written)
    }

    /// Whether a match waits for its deck link.
    pub fn has_pending_links(&self) -> bool {
        !self.links.is_empty()
    }

    /// Link each waiting match to its deck version by exact signature among
    /// the account's versions (its own deck's first). The deck writes were
    /// committed before this is called, so a version that is not there now
    /// will not come: the link is dropped. A linked match's leftover deck is
    /// archived in the same batch, after the link, when the store finds it
    /// unused and untouched. Deck writes held behind a link go into the
    /// batch after it.
    pub fn link(
        &mut self,
        reader: &Connection,
        batch: &mut WriteBatch,
        platform: Platform,
    ) -> Result<(), TrackerError> {
        let links = std::mem::take(&mut self.links);
        for link in links {
            match version_for(reader, &link)? {
                Some(deck_version_id) => {
                    batch.decks.push(DeckWrite::Link {
                        match_token: link.token,
                        deck_version_id,
                    });
                    if let Some(leftover) = link.leftover
                        && leftover != link.deck_id
                    {
                        batch.decks.push(DeckWrite::ArchiveUnused {
                            mtgo_id: leftover,
                            at_ms: batch.now_ms,
                        });
                    }
                }
                None => tracing::warn!(
                    token = %link.token,
                    platform = platform.as_str(),
                    "a match's deck version was not found; not linked"
                ),
            }
        }
        let held = std::mem::take(&mut self.held);
        for (write, link) in held {
            self.push(write, link);
        }
        self.write_into(batch);
        Ok(())
    }
}

/// A deck write's account, `from` renamed to `to` (#862).
pub(crate) fn rename_in_write(write: &mut DeckWrite, from: &str, to: &str) {
    match write {
        DeckWrite::SyncFile { account, .. } | DeckWrite::SyncUse { account, .. } => {
            if account.as_deref() == Some(from) {
                *account = Some(to.to_string());
            }
        }
        DeckWrite::Adopt { account, .. } | DeckWrite::AdoptOrphans { account, .. } => {
            if account == from {
                *account = to.to_string();
            }
        }
        DeckWrite::Link { .. } | DeckWrite::ArchiveUnused { .. } => {}
    }
}

fn rename_in_link(link: &mut PendingLink, from: &str, to: &str) {
    if link.account == from {
        link.account = to.to_string();
    }
}

/// The version a waiting match was played with: among the account's
/// versions with its signature, its own deck's, else the first.
fn version_for(reader: &Connection, link: &PendingLink) -> Result<Option<i64>, TrackerError> {
    let own_deck = deck_by_mtgo_id(reader, &link.deck_id)?.map(|deck| deck.id);
    let versions = account_versions(reader, &link.account)?;
    let mut first = None;
    for version in versions {
        if version.signature != link.signature {
            continue;
        }
        if Some(version.deck_id) == own_deck {
            return Ok(Some(version.id));
        }
        first = first.or(Some(version.id));
    }
    Ok(first)
}

/// Deck rows of `(catalog id, count)` pairs, each count capped.
fn deck_cards(pairs: &[(u32, u32)], sideboard: bool) -> Vec<DeckCard> {
    let mut cards = Vec::new();
    for (catalog_id, quantity) in pairs {
        cards.push(DeckCard {
            catalog_id: *catalog_id,
            quantity: (*quantity).min(MAX_CARD_QUANTITY),
            sideboard,
        });
    }
    cards
}

/// The colour identity a changed file gives its deck. A file without cards
/// keeps the stored one (0.x returns early); otherwise the computed one is
/// stored, NULL included. Shared by the MTGO, Arena and Endstep deck syncs.
pub(super) fn identity_update(
    reader: &Connection,
    cards: &[DeckCard],
) -> Result<IdentityUpdate, TrackerError> {
    if cards.is_empty() {
        return Ok(IdentityUpdate::Keep);
    }
    let catalog_ids: Vec<u32> = cards.iter().map(|card| card.catalog_id).collect();
    let infos = card_infos(reader, &catalog_ids)?;
    let info_of = |catalog_id: u32| {
        infos.get(&catalog_id).map(|card| CardInfo {
            card_type: card.card_type.clone(),
            color_identity: card.color_identity.clone(),
        })
    };
    Ok(IdentityUpdate::Set(decks::color_identity(cards, &info_of)))
}
