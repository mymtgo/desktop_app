//! Copies of cards and spells (decision 0023; CR 707.10). Arena shows a copy
//! as a card object with `isCopy`, made by a `ZoneTransfer` of category
//! `Copy` (the stack to the stack; owner data: Plumb the Forbidden's copies,
//! the transfer's affector the trigger that copies) or found in a zone with
//! no transfer (owner data: a copy put into exile, then cast). Its
//! `parentId` leads to the original, directly or through the ability that
//! made it.
//!
//! A copy is `new copy ID of ORIGINAL NAME owner PLAYER in ZONE [ENTERING]
//! CAUSE`, owned by the player who controls it (CR 707.10, 108.3), with
//! `ENTERING` when it is introduced on the battlefield. Once it is outside
//! the stack and the battlefield it ceases as a state-based action (CR
//! 704.5e): Arena deletes it, and the record writes `cease by rule:sba`.
//!
//! A copy of a permanent spell becomes a token as it resolves and keeps its
//! id (CR 608.3f, decision 0023 item 4): the record moves it from the stack
//! to the battlefield like any permanent spell. When Arena then shows it as
//! a `GameObjectType_Token`, the record tracks it as a token, so it ceases
//! when Arena deletes the token (`TokenDeleted`); shown as a card with
//! `isCopy`, it stays a copy and ceases as copies do. The fixtures and the
//! owner's logs have no copy at all (no `isCopy`), so this follows Arena's
//! shape for every other resolving permanent spell: `ObjectIdChanged` and a
//! `Resolve` zone transfer. Not handled: Arena deleting the copy on the
//! stack and making a new token object instead; the copy would never leave
//! the record's stack (nothing ceases it) and the token would be a `new
//! token` with its own id. That needs a real log first.

use ::mtgr::{Cause, ObjRef, Statement, Zone};

use super::GameWriter;
use super::ids::Kind;
use crate::{GameObject, Transfer};

/// How far up `parentId` the original is looked for (copy, ability, spell).
const PARENT_STEPS: usize = 3;

impl GameWriter<'_> {
    /// The Arena object a transfer moved, under its new id, else under the
    /// id it had before.
    fn moved_object(&self, index: usize, transfer: &Transfer) -> Option<&GameObject> {
        let fold = self.fold;
        let now = fold.states[index].objects.get(&transfer.instance_id);
        let before = transfer.previous_id.and_then(|previous| {
            index
                .checked_sub(1)
                .and_then(|at| fold.states[at].objects.get(&previous))
        });
        now.or(before)
    }

    /// Whether a transfer moves a copy the record has not introduced.
    pub(super) fn is_new_copy(&self, index: usize, transfer: &Transfer) -> bool {
        if self.ids.get(transfer.card).is_some() {
            return false;
        }
        transfer.category == "Copy"
            || self
                .moved_object(index, transfer)
                .is_some_and(|object| object.is_copy)
    }

    /// The original a copy was made from: the first tracked object up its
    /// `parentId`s (the copy's, else the one under its previous id).
    fn original_of(&self, index: usize, transfer: &Transfer) -> Option<ObjRef> {
        let fold = self.fold;
        let mut parents = Vec::new();
        if let Some(object) = fold.states[index].objects.get(&transfer.instance_id) {
            parents.push(object.parent_id);
        }
        if let Some(previous) = transfer.previous_id
            && let Some(object) = index
                .checked_sub(1)
                .and_then(|at| fold.states[at].objects.get(&previous))
        {
            parents.push(object.parent_id);
        }
        for mut parent in parents {
            for _ in 0..PARENT_STEPS {
                if parent == 0 {
                    break;
                }
                let key = fold.identity.key(parent);
                if let Some(tracked) = self.ids.get(key)
                    && !matches!(tracked.kind, Kind::Ability { .. })
                {
                    return self.ids.obj(key);
                }
                let up = [index, index.saturating_sub(1)]
                    .iter()
                    .find_map(|at| fold.states[*at].objects.get(&parent))
                    .map(|object| object.parent_id)
                    .unwrap_or(0);
                parent = up;
            }
        }
        None
    }

    /// Introduce the copy a transfer moves, in `zone`, by `cause`. Returns
    /// whether it was written (`false`: the original is unknown, written as
    /// `incomplete`).
    pub(super) fn introduce_copy(
        &mut self,
        index: usize,
        transfer: &Transfer,
        zone: Zone,
        cause: Cause,
    ) -> bool {
        let Some(of) = self.original_of(index, transfer) else {
            self.write_incomplete("Arena copied an object the record cannot tell".to_string());
            self.untold_copies.insert(transfer.card);
            return false;
        };
        let object = self.moved_object(index, transfer);
        let seat = object
            .map(|object| object.controller_seat)
            .filter(|seat| *seat != 0)
            .unwrap_or(transfer.owner_seat);
        let grp_id = self.card_grp_at(index, transfer.instance_id, transfer.grp_id);
        // A copy of a card the card data does not list is named `?`
        // (decision 0048).
        let name = self.card_name(grp_id);
        // A copy introduced on the battlefield is a permanent there (spec
        // 6.5): as Arena shows it entering when a `Copy` transfer puts it
        // there, else as it was before it left.
        let fold = self.fold;
        let entering = (zone == Zone::Battlefield).then(|| {
            if transfer.category == "Copy" {
                let entered = fold.states[index].objects.get(&transfer.instance_id);
                self.entering(index, entered)
            } else {
                self.entering_before(index, transfer)
            }
        });
        let id = self
            .ids
            .introduce(transfer.card, Kind::Copy, name.clone(), false, zone, seat);
        self.event(
            Statement::NewCopy {
                id,
                of,
                name,
                owner: self.roster.name(seat),
                zone,
                entering: entering.clone(),
            },
            Some(cause),
        );
        if let Some(entering) = entering {
            self.entered(transfer.card, &entering);
        }
        true
    }
}
