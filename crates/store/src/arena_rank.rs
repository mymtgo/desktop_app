//! Arena rank reads: the rank observations of one ladder in log order, the
//! newest season event, the account's ranked matches with their place in the
//! log, the gap claims of manual matches, and whether a gap can be claimed.

use arena_match::rank_line::{MatchPlace, Observation, RankedMatch, is_claimable, rank_line};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::write::decode_raw;
use crate::{BestOf, StoreError};

/// One stored rank reading, with the log place it was read at.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredObservation {
    pub file_id: i64,
    pub byte_offset: u64,
    pub observed_at_ms: i64,
    pub season: u32,
    pub class: String,
    pub level: u32,
    pub step: u32,
    pub won: u32,
    pub lost: u32,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<u32>,
}

/// One of an account's ranked matches, placed in the log when it can be.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedMatchRow {
    pub token: String,
    pub deck_id: Option<i64>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub outcome: String,
    /// The greatest `(file id, byte offset)` of the match's stored events.
    pub log_place: Option<(i64, u64)>,
    /// Where a manual match was claimed to sit in the rank history.
    pub claim: Option<(i64, u64)>,
    /// Bo1 or Bo3 from `game_structure`; `None` when unset or unknown.
    pub best_of: Option<BestOf>,
}

/// The account's readings of one ladder in one season, in log order.
pub fn observations(
    conn: &Connection,
    account_id: i64,
    ladder: &str,
    season: u32,
) -> Result<Vec<StoredObservation>, StoreError> {
    read_observations(conn, account_id, ladder, Some(season))
}

/// The account's readings of one ladder in every season, in log order.
pub fn all_observations(
    conn: &Connection,
    account_id: i64,
    ladder: &str,
) -> Result<Vec<StoredObservation>, StoreError> {
    read_observations(conn, account_id, ladder, None)
}

/// One ladder's readings in log order: one season, or every season for `None`.
fn read_observations(
    conn: &Connection,
    account_id: i64,
    ladder: &str,
    season: Option<u32>,
) -> Result<Vec<StoredObservation>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT arena_log_file_id, byte_offset, observed_at, season, class, level, step,
             matches_won, matches_lost, mythic_percentile, mythic_placement
         FROM arena_rank_observations
         WHERE account_id = ?1 AND ladder = ?2 AND (?3 IS NULL OR season = ?3)
         ORDER BY arena_log_file_id, byte_offset",
    )?;
    let rows = stmt.query_map(params![account_id, ladder, season], stored_observation)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One `arena_rank_observations` row, in the column order the reads select.
fn stored_observation(row: &rusqlite::Row) -> rusqlite::Result<StoredObservation> {
    Ok(StoredObservation {
        file_id: row.get(0)?,
        byte_offset: row.get::<_, i64>(1)? as u64,
        observed_at_ms: row.get(2)?,
        season: row.get(3)?,
        class: row.get(4)?,
        level: row.get(5)?,
        step: row.get(6)?,
        won: row.get(7)?,
        lost: row.get(8)?,
        mythic_percentile: row.get(9)?,
        mythic_placement: row.get(10)?,
    })
}

/// The highest season the account has a reading of on this ladder.
pub fn newest_season(
    conn: &Connection,
    account_id: i64,
    ladder: &str,
) -> Result<Option<u32>, StoreError> {
    let season: Option<u32> = conn.query_row(
        "SELECT MAX(season) FROM arena_rank_observations WHERE account_id = ?1 AND ladder = ?2",
        params![account_id, ladder],
        |row| row.get(0),
    )?;
    Ok(season)
}

/// The newest stored event of `kind` (by file, offset, seq) as its raw JSON.
pub fn latest_arena_event(conn: &Connection, kind: &str) -> Result<Option<String>, StoreError> {
    let row: Option<(Vec<u8>, bool)> = conn
        .query_row(
            "SELECT raw, compressed FROM arena_log_events WHERE kind = ?1
             ORDER BY arena_log_file_id DESC, byte_offset DESC, seq DESC LIMIT 1",
            [kind],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match row {
        Some((raw, compressed)) => Ok(Some(decode_raw(raw, compressed)?)),
        None => Ok(None),
    }
}

/// The account's listed ranked matches, oldest first.
pub fn ranked_matches(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<RankedMatchRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT m.token, v.deck_id, m.started_at, m.ended_at, m.outcome,
             (SELECT e.arena_log_file_id FROM arena_log_events e WHERE e.match_token = m.token
                 ORDER BY e.arena_log_file_id DESC, e.byte_offset DESC LIMIT 1),
             (SELECT e.byte_offset FROM arena_log_events e WHERE e.match_token = m.token
                 ORDER BY e.arena_log_file_id DESC, e.byte_offset DESC LIMIT 1),
             c.arena_log_file_id, c.byte_offset, m.game_structure
         FROM listed_matches m
         LEFT JOIN deck_versions v ON v.id = m.deck_version_id
         LEFT JOIN arena_rank_gap_claims c ON c.match_token = m.token
         WHERE m.account_id = ?1 AND m.match_type = 'ranked'
         ORDER BY m.started_at, m.id",
    )?;
    let rows = stmt.query_map([account_id], |row| {
        let place = |file: Option<i64>, offset: Option<i64>| match (file, offset) {
            (Some(file), Some(offset)) => Some((file, offset as u64)),
            _ => None,
        };
        Ok(RankedMatchRow {
            token: row.get(0)?,
            deck_id: row.get(1)?,
            started_at_ms: row.get(2)?,
            ended_at_ms: row.get(3)?,
            outcome: row.get(4)?,
            log_place: place(row.get(5)?, row.get(6)?),
            claim: place(row.get(7)?, row.get(8)?),
            best_of: row
                .get::<_, Option<String>>(9)?
                .and_then(|text| BestOf::from_key(&text)),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Records (or moves) the place a manual match sits at in the rank history.
pub fn claim_gap(
    tx: &Transaction,
    match_token: &str,
    file_id: i64,
    byte_offset: u64,
    now_ms: i64,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO arena_rank_gap_claims (match_token, arena_log_file_id, byte_offset, claimed_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (match_token) DO UPDATE SET
             arena_log_file_id = excluded.arena_log_file_id,
             byte_offset = excluded.byte_offset,
             claimed_at = excluded.claimed_at",
        params![match_token, file_id, byte_offset as i64, now_ms],
    )?;
    Ok(())
}

/// A stored reading as the rank line reads it.
pub fn observation(stored: StoredObservation) -> Observation {
    Observation {
        file_id: stored.file_id,
        offset: stored.byte_offset,
        at_ms: stored.observed_at_ms,
        class: stored.class,
        level: stored.level,
        step: stored.step,
        won: stored.won,
        lost: stored.lost,
        mythic_percentile: stored.mythic_percentile,
        mythic_placement: stored.mythic_placement,
    }
}

/// Where a ranked match sits: a tracked match at its last stored event, a
/// manual one at its claim, a manual one without a claim nowhere.
pub fn placed(row: &RankedMatchRow) -> Option<RankedMatch> {
    let place = match (row.log_place, row.claim) {
        (Some((file_id, offset)), _) => MatchPlace::Log { file_id, offset },
        (None, Some((file_id, offset))) => MatchPlace::Claim { file_id, offset },
        (None, None) => return None,
    };
    Some(RankedMatch {
        token: row.token.clone(),
        place,
    })
}

/// Whether a manual match can be claimed into the constructed gap ending at
/// this log place: the gap is in the account's newest season's line and is
/// claimable (`arena_match::rank_line::claimable`), as the Add match sheet
/// offers it.
pub fn gap_claimable(
    conn: &Connection,
    account_id: i64,
    file_id: i64,
    byte_offset: u64,
) -> Result<bool, StoreError> {
    let Some(season) = newest_season(conn, account_id, "constructed")? else {
        return Ok(false);
    };
    let points: Vec<Observation> = observations(conn, account_id, "constructed", season)?
        .into_iter()
        .map(observation)
        .collect();
    let mut matches = Vec::new();
    for row in ranked_matches(conn, account_id)? {
        if let Some(found) = placed(&row) {
            matches.push(found);
        }
    }
    let line = rank_line(points, &matches);
    Ok(is_claimable(&line, &matches, file_id, byte_offset))
}

/// Sets the rank a match was played at from the `match_rank_readings` view
/// (migration 0013 holds the rule), clearing it when the match has none.
/// Every write of a match calls this, so a reading stored after the match
/// was first written, but placed before it in the log, is picked up the next
/// time the match is written.
pub(crate) fn set_match_rank(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET (rank_file_id, rank_byte_offset) = (
             SELECT v.rank_file_id, v.rank_byte_offset FROM match_rank_readings v
             WHERE v.match_id = matches.id)
         WHERE id = ?1",
    )?
    .execute([match_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{arena_account, db, hide, insert_manual, move_match_to_arena};
    use crate::write::{COMPRESS_OVER, encode_raw};
    use rusqlite::{Connection, params};

    fn file(conn: &Connection) {
        conn.execute(
            "INSERT OR IGNORE INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4)",
            [],
        )
        .expect("file");
    }

    fn observe(conn: &Connection, account: i64, ladder: &str, season: u32, offset: i64, step: i64) {
        file(conn);
        conn.execute(
            "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at, class,
                 level, step, matches_won, matches_lost, mythic_percentile, mythic_placement,
                 arena_log_file_id, byte_offset)
             VALUES (?1, ?2, ?3, 1000, 'Gold', 2, ?4, 4, 5, NULL, NULL, 1, ?5)",
            params![account, ladder, season, step, offset],
        )
        .expect("observation");
    }

    fn event(conn: &Connection, kind: &str, offset: i64, seq: i64, raw: &str, token: Option<&str>) {
        file(conn);
        let (bytes, compressed) = encode_raw(raw).expect("encode");
        conn.execute(
            "INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind, match_token,
                 logged_at, raw, compressed) VALUES (1, ?1, ?2, ?3, ?4, 1, ?5, ?6)",
            params![offset, seq, kind, token, bytes, compressed],
        )
        .expect("event");
    }

    fn ranked(conn: &Connection, token: &str) {
        insert_manual(conn, token);
        conn.execute(
            "UPDATE matches SET match_type = 'ranked' WHERE token = ?1",
            [token],
        )
        .expect("ranked");
        move_match_to_arena(conn, token);
    }

    #[test]
    fn observations_come_in_log_order() {
        let (_dir, conn) = db("arena-rank-order");
        let account = arena_account(&conn);
        observe(&conn, account, "constructed", 42, 900, 3);
        observe(&conn, account, "constructed", 42, 100, 1);
        observe(&conn, account, "limited", 42, 500, 2);
        observe(&conn, account, "constructed", 41, 50, 0);
        let rows = observations(&conn, account, "constructed", 42).expect("rows");
        let places: Vec<(i64, u64, u32)> = rows
            .iter()
            .map(|row| (row.file_id, row.byte_offset, row.step))
            .collect();
        assert_eq!(places, vec![(1, 100, 1), (1, 900, 3)]);
        assert_eq!(rows[0].class, "Gold");
        assert_eq!((rows[0].won, rows[0].lost), (4, 5));
        assert_eq!(rows[0].mythic_percentile, None);
    }

    #[test]
    fn all_observations_cover_every_season_in_log_order() {
        let (_dir, conn) = db("arena-rank-all");
        let account = arena_account(&conn);
        observe(&conn, account, "constructed", 42, 900, 3);
        observe(&conn, account, "constructed", 41, 50, 0);
        observe(&conn, account, "limited", 42, 500, 2);
        observe(&conn, account, "constructed", 42, 100, 1);
        let rows = all_observations(&conn, account, "constructed").expect("rows");
        let places: Vec<(u64, u32)> = rows
            .iter()
            .map(|row| (row.byte_offset, row.season))
            .collect();
        assert_eq!(places, vec![(50, 41), (100, 42), (900, 42)]);
        assert!(
            all_observations(&conn, account + 1, "constructed")
                .expect("other")
                .is_empty()
        );
    }

    #[test]
    fn newest_season_ignores_other_accounts() {
        let (_dir, conn) = db("arena-rank-season");
        let account = arena_account(&conn);
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qother#99999', 0, 'arena', 'Qother-id')",
            [],
        )
        .expect("other");
        let other: i64 = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = 'Qother#99999'",
                [],
                |r| r.get(0),
            )
            .expect("id");
        observe(&conn, account, "constructed", 41, 100, 1);
        observe(&conn, other, "constructed", 50, 200, 1);
        observe(&conn, account, "limited", 49, 300, 1);
        assert_eq!(
            newest_season(&conn, account, "constructed").expect("s"),
            Some(41)
        );
        assert_eq!(
            newest_season(&conn, account, "limited").expect("s"),
            Some(49)
        );
        assert_eq!(newest_season(&conn, other + 1, "limited").expect("s"), None);
    }

    #[test]
    fn latest_season_event_is_decoded_raw() {
        let (_dir, conn) = db("arena-rank-event");
        assert_eq!(latest_arena_event(&conn, "season").expect("none"), None);
        let long = format!("{{\"pad\":\"{}\"}}", "x".repeat(COMPRESS_OVER + 10));
        event(&conn, "season", 100, 0, "{\"old\":1}", None);
        event(&conn, "rank", 150, 0, "{\"rank\":1}", None);
        assert_eq!(
            latest_arena_event(&conn, "season").expect("plain"),
            Some("{\"old\":1}".to_string())
        );
        event(&conn, "season", 200, 0, &long, None);
        let compressed: i64 = conn
            .query_row(
                "SELECT compressed FROM arena_log_events WHERE byte_offset = 200",
                [],
                |r| r.get(0),
            )
            .expect("flag");
        assert_eq!(compressed, 1, "the long one is stored compressed");
        assert_eq!(
            latest_arena_event(&conn, "season").expect("long"),
            Some(long)
        );
    }

    #[test]
    fn ranked_matches_place_tracked_matches_by_their_last_event() {
        let (_dir, conn) = db("arena-rank-tracked");
        ranked(&conn, "q-tracked");
        event(&conn, "game_info", 100, 0, "{}", Some("q-tracked"));
        event(&conn, "game_info", 700, 1, "{}", Some("q-tracked"));
        event(&conn, "game_info", 700, 0, "{}", Some("q-tracked"));
        event(&conn, "game_info", 300, 0, "{}", Some("q-other"));
        let account = arena_account(&conn);
        let rows = ranked_matches(&conn, account).expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].token, "q-tracked");
        assert_eq!(rows[0].log_place, Some((1, 700)));
        assert_eq!(rows[0].claim, None);
        assert_eq!(rows[0].outcome, "win");
        assert_eq!(rows[0].started_at_ms, 1000);
        assert_eq!(rows[0].ended_at_ms, Some(5000));
        assert_eq!(rows[0].deck_id, None);
    }

    #[test]
    fn manual_matches_carry_their_claim() {
        let (_dir, mut conn) = db("arena-rank-claim");
        ranked(&conn, "q-manual");
        file(&conn);
        let account = arena_account(&conn);
        let tx = conn.transaction().expect("tx");
        claim_gap(&tx, "q-manual", 1, 400, 9).expect("claim");
        claim_gap(&tx, "q-manual", 1, 800, 10).expect("claim again moves it");
        tx.commit().expect("commit");
        let rows = ranked_matches(&conn, account).expect("rows");
        assert_eq!(rows[0].claim, Some((1, 800)));
        assert_eq!(rows[0].log_place, None);
        let claims: i64 = conn
            .query_row("SELECT COUNT(*) FROM arena_rank_gap_claims", [], |r| {
                r.get(0)
            })
            .expect("count");
        assert_eq!(claims, 1);
    }

    #[test]
    fn deck_comes_through_the_deck_version() {
        let (_dir, conn) = db("arena-rank-deck");
        ranked(&conn, "q-deck");
        conn.execute_batch(
            "INSERT INTO decks (id, mtgo_id, name, format, source, created_at, updated_at, account_id)
                 VALUES (7, 'd7', 'Q deck', 'Standard', 'file', 0, 0, (SELECT id FROM accounts WHERE platform = 'arena'));
             INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (3, 7, 's', 0);
             UPDATE matches SET deck_version_id = 3 WHERE token = 'q-deck';",
        )
        .expect("deck");
        let account = arena_account(&conn);
        assert_eq!(
            ranked_matches(&conn, account).expect("rows")[0].deck_id,
            Some(7)
        );
    }

    #[test]
    fn unranked_matches_are_left_out() {
        let (_dir, conn) = db("arena-rank-unranked");
        ranked(&conn, "q-ranked");
        ranked(&conn, "q-play");
        conn.execute(
            "UPDATE matches SET match_type = 'play' WHERE token = 'q-play'",
            [],
        )
        .expect("play");
        let account = arena_account(&conn);
        let tokens: Vec<String> = ranked_matches(&conn, account)
            .expect("rows")
            .into_iter()
            .map(|row| row.token)
            .collect();
        assert_eq!(tokens, vec!["q-ranked"]);
    }

    #[test]
    fn a_hidden_match_loses_its_claim() {
        let (_dir, mut conn) = db("arena-rank-hidden");
        ranked(&conn, "q-hidden");
        file(&conn);
        let account = arena_account(&conn);
        let tx = conn.transaction().expect("tx");
        claim_gap(&tx, "q-hidden", 1, 400, 9).expect("claim");
        tx.commit().expect("commit");
        hide(&conn, "q-hidden");
        assert!(ranked_matches(&conn, account).expect("rows").is_empty());
    }

    #[test]
    fn a_gap_is_claimable_where_the_constructed_rank_moved_with_no_match() {
        let (_dir, conn) = db("arena-rank-claimable");
        let account = arena_account(&conn);
        observe(&conn, account, "constructed", 42, 300, 1);
        observe(&conn, account, "constructed", 42, 500, 2);
        observe(&conn, account, "constructed", 42, 600, 2);
        observe(&conn, account, "limited", 42, 700, 3);
        assert!(gap_claimable(&conn, account, 1, 500).expect("yes"));
        assert!(!gap_claimable(&conn, account, 1, 300).expect("the first reading"));
        assert!(!gap_claimable(&conn, account, 1, 600).expect("quiet"));
        assert!(!gap_claimable(&conn, account, 1, 700).expect("limited only"));
        assert!(!gap_claimable(&conn, account, 1, 501).expect("no reading"));
        assert!(!gap_claimable(&conn, account + 1, 1, 500).expect("other account"));
    }

    #[test]
    fn only_the_newest_seasons_gaps_are_claimable() {
        let (_dir, conn) = db("arena-rank-claimable-season");
        let account = arena_account(&conn);
        observe(&conn, account, "constructed", 41, 300, 1);
        observe(&conn, account, "constructed", 41, 400, 2);
        observe(&conn, account, "constructed", 42, 500, 1);
        observe(&conn, account, "constructed", 42, 600, 3);
        assert!(!gap_claimable(&conn, account, 1, 400).expect("last season"));
        assert!(gap_claimable(&conn, account, 1, 600).expect("this season"));
    }

    // ---- The rank a ranked match was played at ------------------------------

    /// A complete ranked Arena view of `token` for Qarena#12345.
    fn ranked_view(token: &str, match_type: &str) -> mtgo_match::MatchView {
        let mut view = crate::test_support::view(token, "arena:Qfoe-id", 1_000);
        view.mtgo_match_id = None;
        view.local_username = "Qarena#12345".to_string();
        view.match_type = Some(match_type.to_string());
        view.format = Some("Standard".to_string());
        view.game_structure = Some("bo1".to_string());
        view.games.truncate(1);
        view
    }

    fn arena_event(token: &str, offset: u64) -> crate::arena::NewArenaEvent {
        crate::arena::NewArenaEvent {
            file_id: 1,
            byte_offset: offset,
            seq: 0,
            kind: "room_state",
            match_token: Some(token.to_string()),
            logged_at_ms: 1_000,
            raw: "{}".to_string(),
        }
    }

    fn reading(offset: u64, season: u32, level: u32) -> crate::arena::RankObservation {
        crate::arena::RankObservation {
            account_username: "Qarena#12345".to_string(),
            account_user_id: None,
            ladder: "constructed",
            season,
            observed_at_ms: 500,
            class: "Gold".to_string(),
            level,
            step: 1,
            won: 0,
            lost: 0,
            mythic_percentile: None,
            mythic_placement: None,
            file_id: 1,
            byte_offset: offset,
        }
    }

    fn upsert(token: &str, match_type: &str, revision: u64) -> crate::write::MatchWrite {
        crate::write::MatchWrite::Upsert {
            view: Box::new(ranked_view(token, match_type)),
            revision,
        }
    }

    fn rank_place(conn: &Connection, token: &str) -> Option<(i64, i64)> {
        conn.query_row(
            "SELECT rank_file_id, rank_byte_offset FROM matches WHERE token = ?1",
            [token],
            |row| {
                let file: Option<i64> = row.get(0)?;
                let offset: Option<i64> = row.get(1)?;
                Ok(file.zip(offset))
            },
        )
        .expect("rank place")
    }

    #[test]
    fn a_written_ranked_arena_match_stores_the_reading_before_its_first_event() {
        use crate::write::{WriteBatch, commit};
        let (_dir, mut conn) = db("arena-match-rank");
        arena_account(&conn);
        file(&conn);
        // Readings at 100 and 900; a stale season 41 answer at 300; the
        // ranked and the casual match start at 500, an early one at 50; all
        // in one batch, as the tracker writes them.
        let batch = WriteBatch {
            rank_observations: vec![
                reading(100, 42, 2),
                reading(300, 41, 4),
                reading(900, 42, 1),
            ],
            arena_events: vec![
                arena_event("q-ranked", 500),
                arena_event("q-ranked", 950),
                arena_event("q-casual", 500),
                arena_event("q-early", 50),
            ],
            matches: vec![
                upsert("q-ranked", "ranked", 1),
                upsert("q-casual", "unranked", 1),
                upsert("q-early", "ranked", 1),
            ],
            ..WriteBatch::default()
        };
        commit(&mut conn, &batch).expect("commit");
        assert_eq!(rank_place(&conn, "q-ranked"), Some((1, 100)));
        assert_eq!(
            rank_place(&conn, "q-casual"),
            None,
            "a casual match has none"
        );
        assert_eq!(rank_place(&conn, "q-early"), None, "no reading before it");
    }

    #[test]
    fn a_reading_stored_late_but_placed_before_is_picked_up_on_the_next_write() {
        use crate::write::{WriteBatch, commit};
        let (_dir, mut conn) = db("arena-match-rank-late");
        arena_account(&conn);
        file(&conn);
        let first = WriteBatch {
            rank_observations: vec![reading(100, 42, 2)],
            arena_events: vec![arena_event("q-ranked", 500)],
            matches: vec![upsert("q-ranked", "ranked", 1)],
            ..WriteBatch::default()
        };
        commit(&mut conn, &first).expect("first");
        assert_eq!(rank_place(&conn, "q-ranked"), Some((1, 100)));
        let later = WriteBatch {
            rank_observations: vec![reading(400, 42, 1)],
            matches: vec![upsert("q-ranked", "ranked", 2)],
            ..WriteBatch::default()
        };
        commit(&mut conn, &later).expect("later");
        assert_eq!(rank_place(&conn, "q-ranked"), Some((1, 400)));
    }
}
