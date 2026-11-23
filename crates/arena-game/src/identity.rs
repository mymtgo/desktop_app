//! Card identity: Arena gives a card a new instance id when it changes zone
//! (`AnnotationType_ObjectIdChanged`), so one physical card is known by the
//! first instance id it had.

use std::collections::BTreeMap;

/// One physical card across instance id changes: the first instance id it had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CardKey(pub u32);

/// Maps every changed instance id to its card's key.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    keys: BTreeMap<u32, CardKey>,
}

impl Identity {
    /// The card an instance id belongs to. An id never seen as a new id is
    /// its own card.
    pub fn key(&self, instance_id: u32) -> CardKey {
        self.keys
            .get(&instance_id)
            .copied()
            .unwrap_or(CardKey(instance_id))
    }

    /// Record that `orig_id` became `new_id`: the new id joins the card the
    /// old id belonged to, so repeated changes chain to the first id.
    pub(crate) fn changed(&mut self, orig_id: u32, new_id: u32) {
        let key = self.key(orig_id);
        if new_id != key.0 {
            self.keys.insert(new_id, key);
        }
    }

    /// Undo the change that gave `new_id` its card (a cancelled action).
    pub(crate) fn forget(&mut self, new_id: u32) {
        self.keys.remove(&new_id);
    }
}
