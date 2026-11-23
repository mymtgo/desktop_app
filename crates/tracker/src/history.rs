//! Stored events back into `PreparedEvent`s, exactly as live ingestion prepared
//! them, so a reload or rebuild projects the same result (live equals rebuild).

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use mtgo_protocol::{PreparedEvent, parse_login, prepare_at};
use store::{Connection, StoreError, read};

/// The local usernames of one log file: the inherited one until the first Login
/// line, then the latest Login at or before each event (what `FileContext` knew).
#[derive(Debug, Clone, Default)]
struct FileUsernames {
    inherited: Option<String>,
    logins: Vec<(u64, String)>,
}

impl FileUsernames {
    fn load(conn: &Connection, log_file_id: i64) -> Result<Self, StoreError> {
        let inherited = read::file_inherited_username(conn, log_file_id)?;
        let logins = read::file_logins(conn, log_file_id)?
            .into_iter()
            .filter_map(|(offset, raw)| parse_login(&raw).map(|login| (offset, login.username)))
            .collect();
        Ok(Self { inherited, logins })
    }

    fn at(&self, offset: u64) -> Option<String> {
        self.logins
            .iter()
            .rev()
            .find(|(login_offset, _)| *login_offset <= offset)
            .map(|(_, name)| name.clone())
            .or_else(|| self.inherited.clone())
    }
}

#[derive(Debug, Default)]
pub struct History {
    usernames: HashMap<i64, FileUsernames>,
}

impl History {
    /// The cached local username of a file at `offset`; `None` when the file's
    /// usernames are not cached.
    #[cfg(test)]
    pub(crate) fn cached_username(&self, log_file_id: i64, offset: u64) -> Option<Option<String>> {
        self.usernames
            .get(&log_file_id)
            .map(|names| names.at(offset))
    }

    /// A Login line was stored for this file: its cached usernames are stale.
    pub fn forget_file(&mut self, log_file_id: i64) {
        self.usernames.remove(&log_file_id);
    }

    /// Every stored event of a match, prepared as live ingestion prepared it.
    pub fn match_events(
        &mut self,
        conn: &Connection,
        token: &str,
    ) -> Result<Vec<PreparedEvent>, StoreError> {
        let stored = read::events_for_match(conn, token)?;
        let mut events = Vec::with_capacity(stored.len());
        for event in stored {
            let names = match self.usernames.entry(event.log_file_id) {
                Entry::Occupied(cached) => cached.into_mut(),
                Entry::Vacant(slot) => slot.insert(FileUsernames::load(conn, event.log_file_id)?),
            };
            let username = names.at(event.byte_offset);
            if let Some(prepared) = prepare_at(
                event.log_file_id as u64,
                event.byte_offset,
                event.logged_at_ms,
                &event.raw,
                username,
            ) {
                events.push(prepared);
            }
        }
        Ok(events)
    }
}
