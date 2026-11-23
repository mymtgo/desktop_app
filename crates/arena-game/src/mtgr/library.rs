//! The library family (spec 4.7, decision 0043): Arena reports it in part,
//! so the record lists it in `Partial` and writes what Arena reports.
//!
//! - **Positions of moves.** Arena lists each library top first. A card
//!   moved into a library goes on the `top` or `bottom` when the state shows
//!   it there: every card above it (or below it) in the list was moved in by
//!   the same message. The cards a mulligan puts on the bottom (CR 103.5)
//!   are the common case, in the order Arena moves them, the last one moved
//!   being the bottom card (loss log line 3657).
//!   A card the message alone put between the ends is `top N` (Teferi,
//!   Hero of Dominaria's "third from the top", decision 0045); with fewer
//!   than N other cards Arena shows it at the bottom (CR 401.7), and it is
//!   `bottom`.
//!   Two or more cards put on the bottom in a random order (Arena renumbers
//!   just them with a `Shuffle`) are `bottom ?` (decision 0047), and become
//!   `untracked`; a single one is plain `bottom` and stays tracked. When
//!   Arena renumbers one card left on the bottom without moving it ("put
//!   the other on the bottom"), a tracked one moves to the `bottom`.
//! - **Positions of new cards.** A card first seen as it leaves a library
//!   is introduced where it was, when the perspective player knows it: a
//!   draw or mill takes the top card, and an effect taking the top (or
//!   bottom) cards gives each its place after the ones taken before it.
//!   Not after a search (the rest of the library shuffled before the
//!   resolution ends, usually in the next message). The opponent's cards,
//!   other than a draw or mill, get one only when every card taken from
//!   that end was revealed (public in the library before the message, as
//!   Cavalier of Thorns' top five or explore's top card are, or paired with
//!   a `RevealedCardCreated`) and became public (#599). A card from a
//!   private look (into a hand, or a graveyard by surveil) tells nobody
//!   where it was, and neither does a card from a look Arena does not
//!   report, whose other cards stay hidden in the library.
//! - **Scry** (CR 701.22a): Arena's `Scry` names the cards put on the top
//!   (`topIds`) and on the bottom (`bottomIds`). The scried cards were the
//!   library's top cards: each is introduced there with its position (named
//!   for the local player, who looks at their own card, hidden for the
//!   opponent's), looked at by the library's owner, and the ones put on the
//!   bottom move there, all by the resolving object. Cards kept on top move
//!   only if their order changed.
//! - **Other looks** (decision 0046): Arena reports a look only to the
//!   local player, in the prompt it sends them: a surveil's `GroupReq`, or
//!   a `SelectNReq` choosing from the top cards ("look at the top N"). Those
//!   cards are introduced where they are and looked at by the object that
//!   asks. The opponent's looks are never reported (`Partial`).
//! - **Keyframes** give each tracked library card's position as the record's
//!   own lines leave it (`PositionView`, spec 14.2), so a reader's state and
//!   the keyframe agree.
//! - Searches are not written: Arena never reports one.

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{
    Cause, LibraryEnd, LibraryPlace, Line, MoveAction, ObjRef, Position, Statement, Zone,
};

use super::GameWriter;
use super::abilities::RESOLUTION_COMPLETE;
use super::gaps::LibraryView;
use super::moves::{is_public, zone_of};
use super::outside::{REVEALED_CARD_CREATED, proxy_shows};
use crate::message::SHUFFLE;
use crate::{Annotation, Fold, GameState, Transfer};

/// Arena's zone kind for a library.
pub(super) const LIBRARY: &str = "ZoneType_Library";
pub(super) const SCRY: &str = "AnnotationType_Scry";

/// The instance ids in `seat`'s library, top first.
fn library_of(state: &GameState, seat: u32) -> Option<&Vec<u32>> {
    state
        .zones
        .values()
        .find(|zone| zone.kind == LIBRARY && zone.owner_seat == Some(seat))
        .map(|zone| &zone.object_ids)
}

/// The transfers of state `index`. The fold keeps transfers in state
/// order, so a binary search finds them without a scan of every transfer
/// of the game.
pub(super) fn transfers_of(fold: &Fold, index: usize) -> &[Transfer] {
    let start = fold
        .transfers
        .partition_point(|transfer| transfer.state_index < index);
    let end = fold
        .transfers
        .partition_point(|transfer| transfer.state_index <= index);
    &fold.transfers[start..end]
}

/// The seat whose library holds `instance_id`, and the library.
fn library_holding(state: &GameState, instance_id: u32) -> Option<(u32, &Vec<u32>)> {
    state
        .zones
        .values()
        .filter(|zone| zone.kind == LIBRARY)
        .find(|zone| zone.object_ids.contains(&instance_id))
        .and_then(|zone| Some((zone.owner_seat?, &zone.object_ids)))
}

/// Where a `Shuffle` of state `index` put cards in a random order, when it
/// is no shuffle of a library: Arena puts cards on an end of a library
/// ("the rest on the bottom in a random order", cascade), then renumbers
/// just those cards with a `Shuffle` (three-games log line 12186). Returns
/// the end the renumbered cards are at and how many there are, when they
/// are all at one end of the library as the state shows it and other cards
/// stay there with them. A `Shuffle` of the whole library is a shuffle;
/// renumbered cards that outnumber the library they joined are not (#602,
/// Task 7). The order of two or more is unknown to everyone (spec 7.1,
/// decision 0047), and the shuffle makes them `untracked` (`moves.rs`).
pub(super) fn random_end(
    fold: &Fold,
    index: usize,
    annotation: &Annotation,
) -> Option<(LibraryEnd, usize)> {
    let seat = *annotation.affected_ids.first()?;
    let list = library_of(&fold.states[index], seat)?;
    let new_ids: BTreeSet<u32> = annotation
        .ints("NewIds")
        .into_iter()
        .filter_map(|id| u32::try_from(id).ok())
        .collect();
    let count = new_ids.len();
    if count == 0 || count >= list.len() {
        return None;
    }
    if list[list.len() - count..]
        .iter()
        .all(|id| new_ids.contains(id))
    {
        return Some((LibraryEnd::Bottom, count));
    }
    if list[..count].iter().all(|id| new_ids.contains(id)) {
        return Some((LibraryEnd::Top, count));
    }
    None
}

/// Whether `seat`'s library was searched in state `index`: a `Shuffle` of
/// the whole library follows the message's moves out of it before the
/// resolution ends. Arena sends the `Shuffle` in the same message or a later
/// one: the next for a fetch land (`bo1-wins-and-concede` gsids 259 and
/// 260), two and three later when each player searches (gsid 322, then 324
/// and 325). The look stops at the first later message that moves a card in
/// or out of that library, and at a `ResolutionComplete`.
fn searched(fold: &Fold, index: usize, seat: u32) -> bool {
    let Some(library) = library_of(&fold.states[index], seat) else {
        return false;
    };
    for (at, step) in fold.steps.iter().enumerate().skip(index) {
        let whole_shuffle = step.annotations.iter().any(|annotation| {
            annotation.is(SHUFFLE)
                && annotation.affected_ids.first() == Some(&seat)
                && annotation.ints("OldIds").len() >= library.len()
        });
        if whole_shuffle {
            return true;
        }
        let library_moved = at > index
            && transfers_of(fold, at).iter().any(|transfer| {
                transfer.owner_seat == seat
                    && (transfer.from_zone == LIBRARY || transfer.to_zone == LIBRARY)
            });
        let resolved = step
            .annotations
            .iter()
            .any(|annotation| annotation.is(RESOLUTION_COMPLETE));
        if library_moved || resolved {
            return false;
        }
    }
    false
}

impl GameWriter<'_> {
    /// Where a transfer put a card in its library, when state `index` shows
    /// it (see the module header): `top` or `bottom` when every card beyond
    /// it was moved in by the same message, `bottom ?` for a random order,
    /// and `top N` for a card the message alone put in between (Teferi,
    /// Hero of Dominaria: "third from the top", decision 0045).
    pub(super) fn library_end(&self, index: usize, transfer: &Transfer) -> Option<LibraryPlace> {
        let fold = self.fold;
        let list = library_of(&fold.states[index], transfer.owner_seat)?;
        if let Some((end, count)) = self.random_placement(index, transfer) {
            return match (end, count) {
                // One card put there randomly is in a known place.
                (LibraryEnd::Bottom, 1) => Some(LibraryPlace::Bottom),
                (LibraryEnd::Bottom, _) => Some(LibraryPlace::RandomBottom),
                (LibraryEnd::Top, 1) => Some(LibraryPlace::Top),
                // No `top ?` (decision 0047, item 4): an unknown position.
                (LibraryEnd::Top, _) => None,
            };
        }
        let at = list.iter().position(|id| *id == transfer.instance_id)?;
        let same_library: Vec<&Transfer> = transfers_of(fold, index)
            .iter()
            .filter(|other| {
                other.owner_seat == transfer.owner_seat
                    && (other.to_zone == LIBRARY || other.from_zone == LIBRARY)
            })
            .collect();
        let moved_in: BTreeSet<u32> = same_library
            .iter()
            .filter(|other| other.to_zone == LIBRARY)
            .map(|other| other.instance_id)
            .collect();
        let on_top = list[..at].iter().all(|id| moved_in.contains(id));
        let on_bottom = list[at + 1..].iter().all(|id| moved_in.contains(id));
        match (on_top, on_bottom) {
            (true, false) => Some(LibraryPlace::Top),
            (false, true) => Some(LibraryPlace::Bottom),
            // Between the ends: the list gives the place when no other card
            // entered or left that library in the message. The card is not
            // the bottom one, so at least N other cards are in the library
            // (check 19).
            (false, false) if same_library.len() == 1 => {
                Some(LibraryPlace::FromTop(u32::try_from(at + 1).ok()?))
            }
            _ => None,
        }
    }

    /// Cards put on an end of a library in a random order (cascade, "the
    /// rest on the bottom in a random order"): the end and the number of
    /// cards of the `Shuffle` that renumbered the transfer's card
    /// (`random_end`).
    fn random_placement(&self, index: usize, transfer: &Transfer) -> Option<(LibraryEnd, usize)> {
        let fold = self.fold;
        let annotation = fold.steps[index].annotations.iter().find(|annotation| {
            annotation.is(SHUFFLE)
                && annotation.affected_ids.first() == Some(&transfer.owner_seat)
                && annotation
                    .ints("OldIds")
                    .contains(&i64::from(transfer.instance_id))
        })?;
        random_end(fold, index, annotation)
    }

    /// The position a card had in its library as a transfer takes it out,
    /// when the perspective player knows it (spec 6.5, decision 0019):
    /// - a draw or a mill takes the top card (CR 121.2, 701.17a);
    /// - otherwise the card is in the run of cards at the top (or bottom)
    ///   of the library that the same message takes out, as when an effect
    ///   looks at the top cards; its position counts the cards of that run
    ///   taken out before it.
    ///
    /// No position when the library is searched (Arena shuffles the rest
    /// of the library in the same message or a later one: `searched`), or
    /// when a card was put into
    /// that library earlier in the message. The opponent's cards other than
    /// a draw or mill have one only when every card of the run was revealed
    /// (`revealed_leaving`) and became public (Cavalier of Thorns revealing
    /// the top five, explore's top card): a card taken from a private look
    /// (into a hand, or a graveyard by surveil) tells nobody where it was,
    /// and a look Arena does not report, with cards left hidden in the
    /// library, tells nobody where the cards it made public were (#599).
    pub(super) fn leaving_position(
        &self,
        index: usize,
        transfer: &Transfer,
        action: Option<MoveAction>,
    ) -> Option<Position> {
        if matches!(action, Some(MoveAction::Draw | MoveAction::Mill)) {
            return Some(Position {
                end: LibraryEnd::Top,
                n: 1,
            });
        }
        let fold = self.fold;
        let seat = transfer.owner_seat;
        let before = &fold.states[index.checked_sub(1)?];
        let list = library_of(before, seat)?;
        let old_id = |t: &Transfer| t.previous_id.unwrap_or(t.instance_id);
        let at = list.iter().position(|id| *id == old_id(transfer))?;
        let mine: Vec<&Transfer> = transfers_of(fold, index)
            .iter()
            .filter(|t| t.owner_seat == seat && (t.from_zone == LIBRARY || t.to_zone == LIBRARY))
            .collect();
        let me = mine
            .iter()
            .position(|t| std::ptr::eq(*t, transfer))
            .unwrap_or(mine.len());
        if mine[..me].iter().any(|t| t.to_zone == LIBRARY) {
            return None;
        }
        let leaving: BTreeSet<u32> = mine
            .iter()
            .filter(|t| t.from_zone == LIBRARY)
            .map(|t| old_id(t))
            .collect();
        if searched(fold, index, seat) {
            return None;
        }
        let earlier: BTreeSet<u32> = mine[..me]
            .iter()
            .filter(|t| t.from_zone == LIBRARY)
            .map(|t| old_id(t))
            .collect();
        let top_run = list.iter().take_while(|id| leaving.contains(id)).count();
        let bottom_run = list
            .iter()
            .rev()
            .take_while(|id| leaving.contains(id))
            .count();
        let (end, run, from_end) = if at < top_run {
            (LibraryEnd::Top, top_run, &list[..at])
        } else if list.len() - at <= bottom_run {
            (LibraryEnd::Bottom, bottom_run, &list[at + 1..])
        } else {
            return None;
        };
        if seat != self.roster.local_seat {
            let run_ids: &[u32] = match end {
                LibraryEnd::Top => &list[..run],
                LibraryEnd::Bottom => &list[list.len() - run..],
            };
            let all_revealed = run_ids.iter().all(|id| {
                mine.iter().any(|t| {
                    t.from_zone == LIBRARY
                        && old_id(t) == *id
                        && zone_of(&t.to_zone).is_some_and(is_public)
                        && self.revealed_leaving(index, t)
                })
            });
            if !all_revealed {
                return None;
            }
        }
        let gone = from_end.iter().filter(|id| earlier.contains(id)).count();
        let n = from_end.len() - gone + 1;
        Some(Position {
            end,
            n: u32::try_from(n).ok()?,
        })
    }

    /// Whether everyone saw the card `transfer` takes out of a library
    /// while it was there (#599): it was public in that library in the state
    /// before the message (Arena shows a reveal of the top cards that way:
    /// Cavalier of Thorns, loss log line 6218), or a `RevealedCardCreated`
    /// of the message pairs with this move and no other (`outside.rs`).
    fn revealed_leaving(&self, index: usize, transfer: &Transfer) -> bool {
        let fold = self.fold;
        let old_id = transfer.previous_id.unwrap_or(transfer.instance_id);
        if let Some(before) = index.checked_sub(1).map(|at| &fold.states[at])
            && let Some(object) = before.objects.get(&old_id)
            && object.public
            && before.zone_kind(object.zone_id) == LIBRARY
        {
            return true;
        }
        let state = &fold.states[index];
        let moved = transfers_of(fold, index);
        fold.steps[index]
            .annotations
            .iter()
            .filter(|annotation| annotation.is(REVEALED_CARD_CREATED))
            .filter_map(|annotation| state.objects.get(&annotation.affector_id))
            .any(|proxy| {
                let shown: Vec<&Transfer> = moved
                    .iter()
                    .filter(|other| proxy_shows(state, proxy, other))
                    .collect();
                matches!(shown.as_slice(), [only] if std::ptr::eq(*only, transfer))
            })
    }

    /// `Scry`: the scried cards introduced on top, looked at, and the ones
    /// put on the bottom moved there (see the module header).
    pub(super) fn write_scry(&mut self, index: usize, annotation: &Annotation) {
        let fold = self.fold;
        let to_ids = |key: &str| -> Vec<u32> {
            annotation
                .ints(key)
                .into_iter()
                .filter_map(|id| u32::try_from(id).ok())
                .collect()
        };
        let tops = to_ids("topIds");
        let bottoms = to_ids("bottomIds");
        let state = &fold.states[index];
        let before = index.checked_sub(1).map(|at| &fold.states[at]);
        // The library each card was in, as it was before the scry.
        let mut scried: Vec<(usize, u32, u32)> = Vec::new();
        for instance_id in tops.iter().chain(&bottoms) {
            let held = before
                .and_then(|state| library_holding(state, *instance_id))
                .or_else(|| library_holding(state, *instance_id));
            let Some((seat, list)) = held else {
                continue;
            };
            let at = list
                .iter()
                .position(|id| id == instance_id)
                .unwrap_or(usize::MAX);
            scried.push((at, *instance_id, seat));
        }
        scried.sort_unstable();
        let count = scried.len();
        let cause = self.object_cause(annotation.affector_id);
        let mut looked: Vec<(u32, ObjRef)> = Vec::new();
        for (at, instance_id, seat) in scried {
            let position = (at < count).then(|| Position {
                end: LibraryEnd::Top,
                n: u32::try_from(at + 1).unwrap_or(1),
            });
            if let Some(object) = self.look_at(index, instance_id, seat, position, &cause) {
                looked.push((instance_id, object));
            }
        }
        // Bottom cards in the order the state lists them, the bottom one
        // last; kept cards moved only when their order changed (top one
        // last).
        let after = |instance_id: u32| {
            library_holding(state, instance_id)
                .and_then(|(_, list)| list.iter().position(|id| *id == instance_id))
        };
        let mut moves: Vec<(u32, ObjRef, LibraryPlace)> = Vec::new();
        let mut bottom: Vec<(usize, u32)> = bottoms
            .iter()
            .filter_map(|id| Some((after(*id)?, *id)))
            .collect();
        bottom.sort_unstable();
        for (_, instance_id) in bottom {
            if let Some((_, object)) = looked.iter().find(|(id, _)| *id == instance_id) {
                moves.push((instance_id, object.clone(), LibraryPlace::Bottom));
            }
        }
        let kept: Vec<u32> = looked
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| tops.contains(id))
            .collect();
        let mut kept_after: Vec<(usize, u32)> = kept
            .iter()
            .filter_map(|id| Some((after(*id)?, *id)))
            .collect();
        kept_after.sort_unstable();
        let reordered: Vec<u32> = kept_after.iter().map(|(_, id)| *id).collect();
        if reordered.len() == kept.len() && reordered != kept {
            for instance_id in reordered.into_iter().rev() {
                if let Some((_, object)) = looked.iter().find(|(id, _)| *id == instance_id) {
                    moves.push((instance_id, object.clone(), LibraryPlace::Top));
                }
            }
        }
        for (instance_id, object, place) in moves {
            self.event(
                Statement::Move {
                    object,
                    from: Zone::Library,
                    to: Zone::Library,
                    action: None,
                    position: Some(place),
                    entering: None,
                    with: None,
                },
                Some(cause.clone()),
            );
            self.ids
                .moved(fold.identity.key(instance_id), Zone::Library);
        }
    }

    /// The library card `instance_id` of `seat`, looked at by its owner
    /// (decision 0020): introduced at `position` when the record does not
    /// track it yet (named for the local player, hidden for the opponent).
    /// Returns the object, or `None` when the record has it elsewhere.
    fn look_at(
        &mut self,
        index: usize,
        instance_id: u32,
        seat: u32,
        position: Option<Position>,
        cause: &Cause,
    ) -> Option<ObjRef> {
        let key = self.fold.identity.key(instance_id);
        let object = match self.ids.get(key) {
            Some(tracked) if tracked.zone == Zone::Library => self.ids.obj(key)?,
            Some(_) => return None,
            None => {
                let grp_id = self.scried_grp_id(index, instance_id);
                let name = self.visible_name(grp_id, seat, Zone::Library);
                self.introduce_at(key, name, seat, Zone::Library, position)
            }
        };
        let player = self.roster.name(seat);
        self.event(
            Statement::Look {
                object: object.clone(),
                player,
            },
            Some(cause.clone()),
        );
        Some(object)
    }

    /// Looks Arena reports outside a scry (decision 0046): the cards a
    /// prompt in state `index` showed the local player (a surveil, "look
    /// at the top N"), when they are the top cards of their own library.
    /// Each is introduced where it is when the record does not track it,
    /// then looked at, by the object that asks. Cards in another zone (a
    /// mulligan's bottom cards) or in the opponent's library give none, and
    /// a prompt sent again looks at nothing new.
    pub(super) fn write_shown_cards(&mut self, index: usize) {
        let fold = self.fold;
        let seat = self.roster.local_seat;
        let Some(list) = library_of(&fold.states[index], seat) else {
            return;
        };
        for shown in &fold.steps[index].shown {
            let count = shown.instance_ids.len();
            let mut places: Vec<(usize, u32)> = Vec::new();
            for instance_id in &shown.instance_ids {
                match list.iter().position(|id| id == instance_id) {
                    Some(at) if at < count => places.push((at, *instance_id)),
                    _ => break,
                }
            }
            if places.len() != count {
                continue;
            }
            places.sort_unstable();
            let cause = self.object_cause(shown.source_id);
            for (at, instance_id) in places {
                let key = fold.identity.key(instance_id);
                if !self.looked.insert((key, shown.source_id, index)) {
                    continue;
                }
                let Ok(n) = u32::try_from(at + 1) else {
                    continue;
                };
                let position = Position {
                    end: LibraryEnd::Top,
                    n,
                };
                self.look_at(index, instance_id, seat, Some(position), &cause);
            }
        }
    }

    /// The grpId a scried card shows, from the state of the scry or one
    /// before it (0 when no state shows it).
    fn scried_grp_id(&self, index: usize, instance_id: u32) -> u32 {
        let fold = self.fold;
        for at in (0..=index).rev() {
            if fold.states[at].objects.contains_key(&instance_id) {
                return self.card_grp_at(at, instance_id, 0);
            }
        }
        0
    }
}

/// Each tracked library card's position as a reader replays the record's
/// lines (spec 14.2): set by `new` lines and keyframes, and by moves into a
/// library (`top`, `top N`, `bottom`, `bottom ?`); a card leaving from a
/// known position (or the top, for a draw or mill) moves the ones beyond
/// it closer, and a card leaving or entering at an unknown position makes
/// every position in that library unknown. A shuffle ends them all.
#[derive(Debug, Default)]
pub(super) struct PositionView {
    /// Lines already applied.
    applied: usize,
    /// Each object's owner, by id.
    owners: BTreeMap<u32, String>,
    /// Each object in a library, by id: its owner and its position.
    in_library: BTreeMap<u32, (String, Option<Position>)>,
    /// Library sizes, for `top N` (a card counted from the bottom is above
    /// the new card when its place from the top is less than N).
    sizes: LibraryView,
}

impl PositionView {
    /// Apply the lines written since the last call.
    pub fn catch_up(&mut self, lines: &[Line]) {
        for line in &lines[self.applied..] {
            self.apply(&line.statement);
            self.sizes.apply(&line.statement);
        }
        self.applied = lines.len();
    }

    /// An object's library position, when the lines give it.
    pub fn position(&self, id: u32) -> Option<Position> {
        self.in_library.get(&id).and_then(|(_, position)| *position)
    }

    fn apply(&mut self, statement: &Statement) {
        match statement {
            Statement::Keyframe => self.in_library.clear(),
            Statement::KeyframeObject {
                object,
                zone: Zone::Library,
                player: Some(owner),
                position,
                ..
            } => {
                self.owners.insert(object.id, owner.clone());
                self.in_library
                    .insert(object.id, (owner.clone(), *position));
            }
            Statement::NewCard {
                object,
                owner,
                zone,
                position,
                ..
            } => self.created(object.id, owner, *zone, *position),
            Statement::NewHidden {
                id,
                owner,
                zone,
                position,
            } => self.created(*id, owner, *zone, *position),
            Statement::NewToken { object, owner, .. } => {
                self.owners.insert(object.id, owner.clone());
            }
            Statement::NewCopy { id, owner, .. } => {
                self.owners.insert(*id, owner.clone());
            }
            Statement::Move {
                object,
                from,
                to,
                action,
                position,
                ..
            } => {
                if *from == Zone::Library {
                    let takes_top = matches!(action, Some(MoveAction::Draw | MoveAction::Mill));
                    self.leave(object.id, takes_top);
                }
                if *to == Zone::Library {
                    // The library's size before the line, less this card
                    // when it was already in it.
                    let others = self.owners.get(&object.id).and_then(|owner| {
                        let size = self.sizes.size(owner)?;
                        Some(if *from == Zone::Library {
                            size - 1
                        } else {
                            size
                        })
                    });
                    self.enter(object.id, *position, others);
                }
            }
            Statement::Play { object, from, .. } | Statement::Cast { object, from, .. } => {
                if *from == Zone::Library {
                    self.leave(object.id, false);
                }
            }
            Statement::Shuffle { player } => {
                self.in_library.retain(|_, (owner, _)| owner != player);
            }
            Statement::Untracked { object } | Statement::Cease { object } => {
                self.in_library.remove(&object.id);
            }
            _ => {}
        }
    }

    /// A `new` line: in a library, the card comes from the untracked pool,
    /// so no other position changes.
    fn created(&mut self, id: u32, owner: &str, zone: Zone, position: Option<Position>) {
        self.owners.insert(id, owner.to_string());
        if zone == Zone::Library {
            self.in_library.insert(id, (owner.to_string(), position));
        }
    }

    fn leave(&mut self, id: u32, takes_top: bool) {
        let Some((owner, position)) = self.in_library.remove(&id) else {
            return;
        };
        let left = position.or_else(|| {
            takes_top.then_some(Position {
                end: LibraryEnd::Top,
                n: 1,
            })
        });
        for (other_owner, other) in self.in_library.values_mut() {
            if *other_owner != owner {
                continue;
            }
            match (left, other.as_mut()) {
                (None, _) => *other = None,
                (Some(left), Some(other)) if other.end == left.end && other.n > left.n => {
                    other.n -= 1;
                }
                _ => {}
            }
        }
    }

    /// A card put into a library at `place`; `others` is the number of
    /// other cards in it, when known.
    fn enter(&mut self, id: u32, place: Option<LibraryPlace>, others: Option<i64>) {
        let Some(owner) = self.owners.get(&id).cloned() else {
            return;
        };
        for (other_owner, other) in self.in_library.values_mut() {
            if *other_owner != owner {
                continue;
            }
            let Some(known) = other.as_mut() else {
                continue;
            };
            match place {
                None => *other = None,
                Some(LibraryPlace::Top) if known.end == LibraryEnd::Top => known.n += 1,
                Some(LibraryPlace::Bottom | LibraryPlace::RandomBottom)
                    if known.end == LibraryEnd::Bottom =>
                {
                    known.n += 1;
                }
                Some(LibraryPlace::FromTop(n)) => match known.end {
                    LibraryEnd::Top if known.n >= n => known.n += 1,
                    LibraryEnd::Top => {}
                    // From the top, a card at `bottom M` is `others - M + 1`.
                    LibraryEnd::Bottom => match others {
                        Some(others) if others - i64::from(known.n) + 1 < i64::from(n) => {
                            known.n += 1;
                        }
                        Some(_) => {}
                        None => *other = None,
                    },
                },
                Some(_) => {}
            }
        }
        let position = match place {
            Some(LibraryPlace::Top) => Some(Position {
                end: LibraryEnd::Top,
                n: 1,
            }),
            Some(LibraryPlace::FromTop(n)) => Some(Position {
                end: LibraryEnd::Top,
                n,
            }),
            Some(LibraryPlace::Bottom) => Some(Position {
                end: LibraryEnd::Bottom,
                n: 1,
            }),
            Some(LibraryPlace::RandomBottom) | None => None,
        };
        self.in_library.insert(id, (owner, position));
    }
}
