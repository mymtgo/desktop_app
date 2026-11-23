//! MTGO's verdict on log matches (#454): the rows its game history is
//! checked against, and the write that stores a verdict
//! (`matches.history_outcome`). The verdict is also written as the outcome,
//! and the projection's upsert keeps it there (`UPSERT_MATCH`), so a rebuild
//! never puts back a derived outcome MTGO disagrees with. A manual match and
//! a corrected one are never touched: the player's answer wins.

use mtgo_match::Outcome;
use rusqlite::{Connection, Transaction, params};

use crate::StoreError;
use crate::names::outcome_str;

/// A stored log match the history can give a verdict on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryCheckRow {
    pub token: String,
    pub mtgo_match_id: u64,
    /// The local account's name.
    pub username: String,
    /// The stored outcome: `win`, `loss`, `draw` or `unknown`.
    pub outcome: String,
    /// The verdict already stored, if any.
    pub history_outcome: Option<String>,
    pub revision: u64,
}

/// Store MTGO's verdict on one match as its verdict and its outcome, at
/// `revision` (a new revision, so the sync pushes the change).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryOutcome {
    pub token: String,
    /// `Win` or `Loss`.
    pub outcome: Outcome,
    pub revision: u64,
}

/// Every log match with an MTGO match id that the player has not corrected.
pub fn history_check_rows(conn: &Connection) -> Result<Vec<HistoryCheckRow>, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT m.token, m.mtgo_match_id, a.username, m.outcome, m.history_outcome, m.revision
         FROM matches m JOIN accounts a ON a.id = m.account_id
         WHERE m.source = 'log' AND m.corrected_at IS NULL AND m.mtgo_match_id IS NOT NULL
         ORDER BY m.id",
    )?;
    let rows = statement.query_map([], |row| {
        let mtgo_match_id: i64 = row.get(1)?;
        let revision: i64 = row.get(5)?;
        Ok(HistoryCheckRow {
            token: row.get(0)?,
            mtgo_match_id: mtgo_match_id as u64,
            username: row.get(2)?,
            outcome: row.get(3)?,
            history_outcome: row.get(4)?,
            revision: revision as u64,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Write one verdict. A manual or corrected row is left alone.
pub(crate) fn apply_history_outcome(
    tx: &Transaction,
    write: &HistoryOutcome,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET history_outcome = ?2, outcome = ?2, revision = ?3
         WHERE token = ?1 AND source = 'log' AND corrected_at IS NULL",
    )?
    .execute(params![
        write.token,
        outcome_str(write.outcome),
        write.revision as i64
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::write::{MatchWrite, WriteBatch, commit};

    fn upsert(conn: &mut Connection, outcome: Outcome, revision: u64) {
        let mut drawn = view(TOKEN, "Qopponent", 1_000);
        drawn.outcome = outcome;
        commit(
            conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(drawn),
                    revision,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("upsert");
    }

    fn verdict(conn: &mut Connection, outcome: Outcome, revision: u64) {
        commit(
            conn,
            &WriteBatch {
                history_outcomes: vec![HistoryOutcome {
                    token: TOKEN.to_string(),
                    outcome,
                    revision,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("verdict");
    }

    fn stored(conn: &Connection) -> (String, Option<String>, i64) {
        conn.query_row(
            "SELECT outcome, history_outcome, revision FROM matches",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("row")
    }

    #[test]
    fn a_verdict_is_the_outcome_and_a_rebuild_keeps_it() {
        let (_dir, mut conn) = db("history-verdict");
        upsert(&mut conn, Outcome::Draw, 3);
        let rows = history_check_rows(&conn).expect("rows");
        assert_eq!(
            rows,
            vec![HistoryCheckRow {
                token: TOKEN.to_string(),
                mtgo_match_id: 99,
                username: "Qlocal".to_string(),
                outcome: "draw".to_string(),
                history_outcome: None,
                revision: 3,
            }]
        );

        verdict(&mut conn, Outcome::Win, 4);
        assert_eq!(
            stored(&conn),
            ("win".to_string(), Some("win".to_string()), 4)
        );

        // The projection derives the draw again: MTGO's verdict stays.
        upsert(&mut conn, Outcome::Draw, 5);
        assert_eq!(
            stored(&conn),
            ("win".to_string(), Some("win".to_string()), 5)
        );
    }

    #[test]
    fn a_corrected_match_keeps_the_player_s_outcome() {
        let (_dir, mut conn) = db("history-verdict-corrected");
        upsert(&mut conn, Outcome::Draw, 3);
        conn.execute("UPDATE matches SET outcome = 'loss', corrected_at = 9", [])
            .expect("correct");
        assert!(history_check_rows(&conn).expect("rows").is_empty());
        verdict(&mut conn, Outcome::Win, 4);
        assert_eq!(stored(&conn), ("loss".to_string(), None, 3));
    }
}
