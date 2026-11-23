//! Arena course snapshots and match event names on their way to the store
//! (Arena limited runs spec section 2).
//!
//! A courses response names no account (trap 6): it belongs to the account
//! logged in when it was read. One read before any login is held until an
//! account is known. A snapshot equal to the account's previous one is not
//! stored again; the previous one is read back from the store the first time
//! an account needs it, so a restart does not store a repeat.
//!
//! Courses are not log events, so neither a rebuild nor the file (read past
//! them) gives held snapshots back after a restart. They are kept in the
//! `ARENA_HELD_COURSES` setting, written in the batch that records the read
//! progress past them, and read back at startup.

use std::collections::HashMap;

use arena_protocol::{ArenaEvent, Course, parse_event_name};
use cards::{ARENA_ID_OFFSET, GrpId, arena_catalog_id};
use store::arena_courses::{CourseObservation, latest_snapshot};
use store::{Connection, StoreError, read};

/// The settings key of the held snapshots: a JSON list of
/// `[file id, byte offset, observed at ms, courses]`.
pub(super) const ARENA_HELD_COURSES: &str = "arena_held_courses";

/// One courses response read before it could be stored.
struct HeldSnapshot {
    file_id: i64,
    byte_offset: u64,
    observed_at_ms: i64,
    courses: Vec<Course>,
}

/// How a held snapshot is kept in the setting.
type StoredHeld = (i64, u64, i64, Vec<Course>);

#[derive(Default)]
pub(super) struct CourseLog {
    /// Snapshots waiting for an account, in log order.
    held: Vec<HeldSnapshot>,
    /// The held list changed since the setting was last queued.
    held_changed: bool,
    /// Each account's newest snapshot, by username, once looked up.
    previous: HashMap<String, Vec<CourseObservation>>,
}

impl CourseLog {
    /// The log with the snapshots a previous run held, from the stored
    /// setting. A value that does not read back is dropped with a warning.
    pub(super) fn restore(reader: &Connection) -> Result<Self, StoreError> {
        let mut log = Self::default();
        let stored = read::settings(reader)?
            .into_iter()
            .find(|(key, _)| key == ARENA_HELD_COURSES);
        if let Some((_, value)) = stored {
            match serde_json::from_str::<Vec<StoredHeld>>(&value) {
                Ok(list) => {
                    for (file_id, byte_offset, observed_at_ms, courses) in list {
                        log.hold(file_id, byte_offset, observed_at_ms, courses);
                    }
                    log.held_changed = false;
                }
                Err(error) => {
                    tracing::warn!(%error, "held Arena course snapshots do not read back; dropped");
                }
            }
        }
        Ok(log)
    }

    /// The setting row to store with this batch, when the held list changed.
    pub(super) fn take_setting(&mut self) -> Option<(String, String)> {
        if !self.held_changed {
            return None;
        }
        self.held_changed = false;
        let list: Vec<StoredHeld> = self
            .held
            .iter()
            .map(|held| {
                (
                    held.file_id,
                    held.byte_offset,
                    held.observed_at_ms,
                    held.courses.clone(),
                )
            })
            .collect();
        match serde_json::to_string(&list) {
            Ok(value) => Some((ARENA_HELD_COURSES.to_string(), value)),
            Err(error) => {
                tracing::warn!(%error, "held Arena course snapshots could not be kept");
                None
            }
        }
    }

    /// Keep a courses response until `release` stores it.
    pub(super) fn hold(
        &mut self,
        file_id: i64,
        byte_offset: u64,
        observed_at_ms: i64,
        courses: Vec<Course>,
    ) {
        self.held.push(HeldSnapshot {
            file_id,
            byte_offset,
            observed_at_ms,
            courses,
        });
        self.held_changed = true;
    }

    pub(super) fn has_held(&self) -> bool {
        !self.held.is_empty()
    }

    /// Whether a held snapshot was read from `login_file` or an earlier file.
    pub(super) fn has_held_up_to(&self, login_file: i64) -> bool {
        self.held.iter().any(|held| held.file_id <= login_file)
    }

    /// The rows of every held snapshot read from `login_file` or an earlier
    /// file, for `username`'s account, leaving out each one equal to the
    /// snapshot before it. Snapshots of a later file stay held.
    pub(super) fn release(
        &mut self,
        reader: &Connection,
        username: &str,
        login_file: i64,
    ) -> Result<Vec<CourseObservation>, StoreError> {
        let mut rows = Vec::new();
        let (ready, waiting): (Vec<HeldSnapshot>, Vec<HeldSnapshot>) =
            std::mem::take(&mut self.held)
                .into_iter()
                .partition(|held| held.file_id <= login_file);
        self.held = waiting;
        self.held_changed |= !ready.is_empty();
        for held in ready {
            let snapshot = snapshot_rows(username, held);
            let previous = self.previous_snapshot(reader, username)?;
            if same_contents(previous, &snapshot) {
                continue;
            }
            rows.extend(snapshot.iter().cloned());
            self.previous.insert(username.to_string(), snapshot);
        }
        Ok(rows)
    }

    /// An account renamed (#862): its newest snapshot is the new name's, so
    /// an equal snapshot under the new name is not stored again.
    pub(super) fn rename_account(&mut self, from: &str, to: &str) {
        if let Some(previous) = self.previous.remove(from) {
            self.previous.insert(to.to_string(), previous);
        }
    }

    fn previous_snapshot(
        &mut self,
        reader: &Connection,
        username: &str,
    ) -> Result<&Vec<CourseObservation>, StoreError> {
        if !self.previous.contains_key(username) {
            let stored = match read::account_id(reader, username)? {
                Some(account_id) => latest_snapshot(reader, account_id)?,
                None => Vec::new(),
            };
            self.previous.insert(username.to_string(), stored);
        }
        Ok(&self.previous[username])
    }
}

/// A snapshot's rows, by course id: one per course, or one marker row with
/// an empty course id when it has none.
fn snapshot_rows(username: &str, held: HeldSnapshot) -> Vec<CourseObservation> {
    let row = |course: Course| CourseObservation {
        file_id: held.file_id,
        byte_offset: held.byte_offset as i64,
        observed_at_ms: held.observed_at_ms,
        account_username: username.to_string(),
        course_id: course.course_id,
        event_name: course.event_name,
        module: course.module,
        wins: course.wins,
        losses: course.losses,
        deck_id: course.deck_id,
        deck_name: course.deck_name,
        main: catalog_counts(&course.main),
        side: catalog_counts(&course.side),
        pool: course
            .pool
            .iter()
            .filter(|grp| **grp < ARENA_ID_OFFSET)
            .map(|grp| arena_catalog_id(GrpId(*grp)))
            .collect(),
    };
    let mut rows: Vec<CourseObservation> = held.courses.into_iter().map(row).collect();
    rows.sort_by(|a, b| a.course_id.cmp(&b.course_id));
    if rows.is_empty() {
        rows.push(row(Course {
            course_id: String::new(),
            event_name: String::new(),
            module: String::new(),
            wins: 0,
            losses: 0,
            deck_id: String::new(),
            deck_name: String::new(),
            main: Vec::new(),
            side: Vec::new(),
            pool: Vec::new(),
        }));
    }
    rows
}

/// `(catalog id, copies)` per card. An id that cannot be a grpId (a hostile
/// log) is left out.
fn catalog_counts(cards: &[arena_protocol::CardCount]) -> Vec<(u32, u32)> {
    cards
        .iter()
        .filter(|card| card.grp_id < ARENA_ID_OFFSET)
        .map(|card| (arena_catalog_id(GrpId(card.grp_id)), card.quantity))
        .collect()
}

/// Whether two snapshots list the same courses with the same modules,
/// counts, decks and pools (their log places and times aside).
fn same_contents(a: &[CourseObservation], b: &[CourseObservation]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.course_id == y.course_id
                && x.event_name == y.event_name
                && x.module == y.module
                && x.wins == y.wins
                && x.losses == y.losses
                && x.deck_id == y.deck_id
                && x.deck_name == y.deck_name
                && x.main == y.main
                && x.side == y.side
                && x.pool == y.pool
        })
}

/// A room state's `(match token, event name)`, when it names a match and an
/// event `parse_event_name` reads.
pub(super) fn room_event_name(event: &ArenaEvent) -> Option<(String, String)> {
    let ArenaEvent::RoomState {
        match_id, players, ..
    } = event
    else {
        return None;
    };
    if match_id.is_empty() {
        return None;
    }
    players
        .iter()
        .find(|seat| parse_event_name(&seat.event_id).is_some())
        .map(|seat| (match_id.clone(), seat.event_id.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arena_protocol::CardCount;

    fn course(id: &str, wins: u32) -> Course {
        Course {
            course_id: id.to_string(),
            event_name: "PremierDraft_FRA_20260929".to_string(),
            module: "CreateMatch".to_string(),
            wins,
            losses: 0,
            deck_id: "q-deck".to_string(),
            deck_name: "Draft Deck".to_string(),
            main: vec![CardCount {
                grp_id: 7,
                quantity: 2,
            }],
            side: Vec::new(),
            pool: vec![7, 7, ARENA_ID_OFFSET],
        }
    }

    fn held(byte_offset: u64, courses: Vec<Course>) -> HeldSnapshot {
        HeldSnapshot {
            file_id: 1,
            byte_offset,
            observed_at_ms: 5,
            courses,
        }
    }

    #[test]
    fn rows_take_catalog_ids_and_an_empty_snapshot_is_one_marker_row() {
        let rows = snapshot_rows("Qplayer", held(10, vec![course("b", 1), course("a", 0)]));
        assert_eq!(
            rows.iter()
                .map(|row| row.course_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert_eq!(rows[0].main, vec![(ARENA_ID_OFFSET + 7, 2)]);
        assert_eq!(rows[0].pool, vec![ARENA_ID_OFFSET + 7; 2]);

        let marker = snapshot_rows("Qplayer", held(20, Vec::new()));
        assert_eq!(marker.len(), 1);
        assert_eq!(
            (
                marker[0].course_id.as_str(),
                marker[0].wins,
                marker[0].pool.len()
            ),
            ("", 0, 0)
        );
    }

    #[test]
    fn a_snapshot_equal_but_for_its_place_is_the_same() {
        let first = snapshot_rows("Qplayer", held(10, vec![course("a", 1)]));
        let again = snapshot_rows("Qplayer", held(90, vec![course("a", 1)]));
        let won = snapshot_rows("Qplayer", held(90, vec![course("a", 2)]));
        assert!(same_contents(&first, &again));
        assert!(!same_contents(&first, &won));
        assert!(!same_contents(&[], &first));
    }
}
