//! Read helpers the built-on-read replays share (`arena_replay` and
//! `endstep_replay`): platform neutral, so neither replay owns them.

use rusqlite::{Connection, OptionalExtension};

use crate::StoreError;

/// The game number of each game a record holds, from its `Game` tag, in
/// order. A game without a readable one is left out.
pub(crate) fn written_game_numbers(record: &::mtgr::Record) -> Vec<u32> {
    let mut numbers = Vec::new();
    for game in &record.games {
        let number: Option<u32> = game
            .tags
            .iter()
            .find(|tag| tag.name == "Game")
            .and_then(|tag| tag.value.parse().ok());
        numbers.extend(number);
    }
    numbers
}

/// The archetype assigned to the match's opponent seat or the local seat
/// (any platform).
pub(crate) fn seat_archetype(
    conn: &Connection,
    match_id: i64,
    opponent: bool,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT a.name FROM match_archetypes x
             JOIN archetypes a ON a.id = x.archetype_id
             WHERE x.match_id = ?1 AND x.is_opponent = ?2",
        )?
        .query_row(rusqlite::params![match_id, opponent], |row| row.get(0))
        .optional()?)
}
