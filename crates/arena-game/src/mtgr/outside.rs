//! Reveals, companions and what the record cannot follow (spec sections
//! 7.3, 9.1 and 9.8, decisions 0025 and 0029).
//!
//! - **Reveals**: `RevealedCardCreated` names a proxy object Arena makes
//!   for the revealed card. The card revealed is the one a zone transfer of
//!   the same message (listed before or after it) moves where the proxy is (same zone kind and owner,
//!   and the same grpId when the moved card shows one); when the proxy is
//!   not shown, the one move of a card the record already names. It is
//!   named where it is (introduced there, or identified), then revealed by
//!   the transfer's affector. A reveal that pairs with no single move is
//!   written `incomplete`.
//! - **Companions**: Arena shows a revealed companion as a public card in
//!   its owner's sideboard from the first state. It is introduced
//!   `outside "sideboard"` and revealed by itself after the starting player
//!   is chosen and before the opening hands (starting player's first), and
//!   keyframes list it with `companion` while it stays there. Putting it
//!   into the hand (`UserActionTaken` action type 7, `ActionType_Special`)
//!   is `special companion`, then its move by itself.
//! - **Plot** (CR 116.2k): Arena reports it as `UserActionTaken` action
//!   type 7 on an ability instance of a card that moves from hand to exile
//!   in the same message and gains the plotted designation
//!   (`GainDesignation` type 18; loss log lines 4436, 14048 and 14778 have
//!   examples). It is `special plot`, then its mana abilities and the
//!   payment `by cost:` the card, then its move (decision 0025).
//! - Arena reports a special action's mana before the action, so special
//!   actions are written before the other lines of their message.
//! - **Incomplete**: a move between zones the record has no keyword for
//!   (Limbo, Revealed, Pending, Suppressed) is written `incomplete`, and
//!   the card's id ends (the record no longer knows where it is).

use ::mtgr::{Cause, SpecialAction, Statement, Zone};

use super::GameWriter;
use super::moves::Introduced;
use super::moves::zone_of;
use crate::{Annotation, CardKey, GameObject, GameState, Transfer};

pub(super) const REVEALED_CARD_CREATED: &str = "AnnotationType_RevealedCardCreated";
const SIDEBOARD: &str = "ZoneType_Sideboard";
/// `UserActionTaken` action type for a special action.
const ACTION_SPECIAL: i64 = 7;
const USER_ACTION: &str = super::abilities::USER_ACTION;
const ABILITY_CREATED: &str = super::abilities::ABILITY_CREATED;
const GAIN_DESIGNATION: &str = "AnnotationType_GainDesignation";
/// `GainDesignation` `DesignationType` of a plotted card.
const PLOTTED: i64 = 18;

/// Whether `proxy`, the object a `RevealedCardCreated` names in `state`,
/// can show the card `transfer` moves: it is where the transfer put the
/// card (same zone kind and owner), with the same grpId when the moved card
/// shows one.
pub(super) fn proxy_shows(state: &GameState, proxy: &GameObject, transfer: &Transfer) -> bool {
    transfer.to_zone == state.zone_kind(proxy.zone_id)
        && transfer.owner_seat == proxy.owner_seat
        && (transfer.grp_id == 0 || transfer.grp_id == proxy.card_grp_id())
}

/// A zone kind as the record's incomplete reason shows it.
fn zone_label(kind: &str) -> &str {
    if kind.is_empty() {
        "an unknown zone"
    } else {
        kind
    }
}

impl<'a> GameWriter<'a> {
    /// `incomplete` with a reason.
    pub(super) fn write_incomplete(&mut self, reason: String) {
        self.event(Statement::Incomplete { reason }, None);
    }

    /// A card move between zones the record has no keyword for.
    pub(super) fn unmapped_move(&mut self, transfer: &Transfer) {
        self.write_incomplete(format!(
            "Arena moved a card from {} to {}",
            zone_label(&transfer.from_zone),
            zone_label(&transfer.to_zone)
        ));
        self.ids.end(transfer.card);
    }

    /// The revealed companions, once, in turn order: `new card` outside the
    /// game and `reveal` by itself.
    pub(super) fn write_companions(&mut self) {
        if self.companions_written {
            return;
        }
        self.companions_written = true;
        let fold = self.fold;
        let Some(state) = fold.states.iter().find(|state| {
            state.zones.values().any(|zone| {
                zone.kind == SIDEBOARD
                    && zone
                        .object_ids
                        .iter()
                        .any(|id| state.objects.get(id).is_some_and(|object| object.public))
            })
        }) else {
            return;
        };
        for seat in self.seat_order() {
            for zone in state.zones.values() {
                if zone.kind != SIDEBOARD || zone.owner_seat != Some(seat) {
                    continue;
                }
                for instance_id in &zone.object_ids {
                    let Some(object) = state.objects.get(instance_id) else {
                        continue;
                    };
                    if !object.public {
                        continue;
                    }
                    let grp_id = object.card_grp_id();
                    if grp_id == 0 {
                        continue;
                    }
                    let name = self.named(grp_id);
                    let key = fold.identity.key(*instance_id);
                    let card = self.introduce(key, Some(name), seat, Zone::OutsideSideboard, None);
                    let id = card.id;
                    self.event(Statement::Reveal { object: card }, Some(Cause::Object(id)));
                    if let Some(tracked) = self.ids.get_mut(key) {
                        tracked.companion = true;
                    }
                }
            }
        }
    }

    /// The special actions of state `index`, before its other lines: the
    /// standard writes `special`, then the payment, then the move (decision
    /// 0025), while Arena reports the mana first.
    pub(super) fn write_specials_ahead(&mut self, index: usize) {
        let fold = self.fold;
        for transfer in &fold.transfers {
            if transfer.state_index == index && transfer.from_zone == SIDEBOARD {
                self.write_companion_special(transfer);
            }
        }
        for annotation in &fold.steps[index].annotations {
            if annotation.is(USER_ACTION) && annotation.int("actionType") == Some(ACTION_SPECIAL) {
                for ability_id in &annotation.affected_ids {
                    if let Some(transfer) = self.plotted_transfer(index, *ability_id) {
                        let ability = fold.identity.key(*ability_id);
                        self.write_plot_special(transfer, ability);
                    }
                }
            }
        }
    }

    /// The move of state `index` that plots the card whose ability instance
    /// `ability_id` a special action names: from hand to exile, with the
    /// plotted designation.
    fn plotted_transfer(&self, index: usize, ability_id: u32) -> Option<&'a Transfer> {
        let fold = self.fold;
        let annotations = &fold.steps[index].annotations;
        let source = annotations
            .iter()
            .find(|a| a.is(ABILITY_CREATED) && a.affected_ids.contains(&ability_id))?
            .affector_id;
        let transfer = fold.transfers.iter().find(|transfer| {
            transfer.state_index == index
                && (transfer.instance_id == source || transfer.previous_id == Some(source))
                && transfer.from_zone == "ZoneType_Hand"
                && transfer.to_zone == "ZoneType_Exile"
        })?;
        let plotted = annotations.iter().any(|a| {
            a.is(GAIN_DESIGNATION)
                && a.int("DesignationType") == Some(PLOTTED)
                && a.affected_ids.contains(&transfer.instance_id)
        });
        plotted.then_some(transfer)
    }

    /// `special plot` for a card plotted from its owner's hand, once. The
    /// mana paid into the special action's ability is paid by the card.
    fn write_plot_special(&mut self, transfer: &Transfer, ability: CardKey) {
        if self.specials.contains(&transfer.card) {
            return;
        }
        self.specials.insert(transfer.card);
        self.paid_for.insert(ability, transfer.card);
        // The card is named where the local player knows it; a hidden card
        // is identified by its move to exile, after the payment.
        let seat = transfer.owner_seat;
        let known = self.known_in(seat, Zone::Hand, Zone::Exile);
        let name = if known {
            self.visible_name(transfer.grp_id, seat, Zone::Hand)
        } else {
            None
        };
        let object = self.object_leaving(
            transfer.card,
            name,
            known,
            seat,
            Zone::Hand,
            Introduced::Anywhere,
        );
        self.event(
            Statement::Special {
                object,
                action: SpecialAction::Plot,
            },
            None,
        );
    }

    /// `special companion` for a companion Arena puts into its owner's hand
    /// with a special action, once.
    pub(super) fn write_companion_special(&mut self, transfer: &Transfer) {
        let Some(tracked) = self.ids.get(transfer.card) else {
            return;
        };
        if !tracked.companion
            || tracked.zone != Zone::OutsideSideboard
            || self.specials.contains(&transfer.card)
        {
            return;
        }
        let special = [Some(transfer.instance_id), transfer.previous_id]
            .into_iter()
            .flatten()
            .any(|id| self.actions.get(&id) == Some(&ACTION_SPECIAL));
        if !special {
            return;
        }
        self.specials.insert(transfer.card);
        let Some(object) = self.ids.obj(transfer.card) else {
            return;
        };
        self.event(
            Statement::Special {
                object,
                action: SpecialAction::Companion,
            },
            None,
        );
    }

    /// The transfer of state `index` a `RevealedCardCreated` reveals, with
    /// its annotation's affector: one move where the proxy is, or, with no
    /// proxy shown, the one move of a card the record shows (named or `?`).
    /// Every move of the state counts, written or not: Arena may list the
    /// `ZoneTransfer` before or after the `RevealedCardCreated`.
    fn revealed_transfer(
        &self,
        index: usize,
        annotation: &Annotation,
    ) -> Option<(&'a Transfer, u32)> {
        let fold = self.fold;
        let state = &fold.states[index];
        let written = self.next_transfer;
        let moves = fold
            .transfers
            .iter()
            .enumerate()
            .filter(|(_, transfer)| transfer.state_index == index);
        let proxy = state.objects.get(&annotation.affector_id);
        let candidates: Vec<&'a Transfer> = match proxy {
            Some(proxy) => moves
                .filter(|(at, transfer)| {
                    proxy_shows(state, proxy, transfer)
                        && (*at >= written || self.tied_to_reveal(index, transfer))
                })
                .map(|(_, transfer)| transfer)
                .collect(),
            None => moves
                .map(|(_, transfer)| transfer)
                .filter(|transfer| {
                    self.ids
                        .get(transfer.card)
                        .is_some_and(|tracked| !tracked.hidden)
                })
                .collect(),
        };
        let [transfer] = candidates.as_slice() else {
            return None;
        };
        Some((*transfer, self.transfer_affector(index, transfer)))
    }

    /// The affector of the `ZoneTransfer` annotation of state `index` that
    /// made `transfer` (0 for none).
    fn transfer_affector(&self, index: usize, transfer: &Transfer) -> u32 {
        self.fold.steps[index]
            .annotations
            .iter()
            .find(|a| {
                a.is(crate::message::ZONE_TRANSFER)
                    && a.affected_ids.contains(&transfer.instance_id)
            })
            .map(|a| a.affector_id)
            .unwrap_or(0)
    }

    /// Whether a move already written can be the one a reveal of the same
    /// message shows. A card moved with no grpId and written hidden could
    /// be any hidden card (an opponent's draw beside a reveal of their hand,
    /// as in `brawl/brawl_queue.log` line 3294): pairing it would tell which
    /// object the revealed card is. It pairs only when the object resolving
    /// now made the move, as a reveal-and-move effect does.
    fn tied_to_reveal(&self, index: usize, transfer: &Transfer) -> bool {
        let hidden = self
            .ids
            .get(transfer.card)
            .is_some_and(|tracked| tracked.hidden);
        if transfer.grp_id != 0 || !hidden {
            return true;
        }
        let affector = self.transfer_affector(index, transfer);
        affector != 0 && self.resolving_now.last() == Some(&self.fold.identity.key(affector))
    }

    /// `RevealedCardCreated`: the card is named where it is, then revealed.
    pub(super) fn write_reveal(&mut self, index: usize, annotation: &Annotation) {
        let fold = self.fold;
        let Some((transfer, affector)) = self.revealed_transfer(index, annotation) else {
            self.write_incomplete("Arena revealed a card the record cannot tell".to_string());
            return;
        };
        let Some(from) = zone_of(&transfer.from_zone) else {
            self.write_incomplete(format!(
                "Arena revealed a card in {}",
                zone_label(&transfer.from_zone)
            ));
            return;
        };
        let proxy_grp = fold.states[index]
            .objects
            .get(&annotation.affector_id)
            .map(|object| object.card_grp_id())
            .unwrap_or(0);
        let grp_id = if proxy_grp != 0 {
            proxy_grp
        } else {
            transfer.grp_id
        };
        let key = transfer.card;
        let shown = self.ids.get(key).is_some_and(|tracked| !tracked.hidden);
        if !shown {
            if grp_id == 0 {
                self.write_incomplete("Arena revealed a card with no name".to_string());
                return;
            }
            let name = self.named(grp_id);
            match self.ids.get(key) {
                None => {
                    self.introduce(key, Some(name), transfer.owner_seat, from, None);
                }
                Some(tracked) => {
                    let id = tracked.id;
                    self.identify(key, id, name);
                }
            }
        }
        self.write_companion_special(transfer);
        // A revealed companion is public already: no second reveal.
        if self.ids.get(key).is_some_and(|tracked| tracked.companion) {
            return;
        }
        let Some(object) = self.ids.obj(key) else {
            return;
        };
        let cause = self.cost_or_effect(affector);
        self.event(Statement::Reveal { object }, Some(cause));
    }
}
