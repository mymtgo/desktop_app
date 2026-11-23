-- The rank a ranked Arena match was played at (Arena Bo1 spec, follow-ups
-- agreed 2026-11-06): the constructed reading in arena_rank_observations
-- named by its log place (ladder 'constructed'). NULL for MTGO, casual
-- ('unranked') and ranked matches with no reading before them.
ALTER TABLE matches ADD COLUMN rank_file_id INTEGER;
ALTER TABLE matches ADD COLUMN rank_byte_offset INTEGER;

-- The one rule, used by this fill and by every write of a match
-- (store::arena_rank::set_match_rank): a ranked Arena match's rank is the
-- latest constructed reading of its account before it, in the newest season
-- read before it (a stale answer of an older season never wins). A tracked
-- match is placed by its first stored Arena event, in log order
-- (file id, byte offset); a manual match has only its start time, so the
-- reading must be observed before started_at. The first event is joined
-- once per match (f), not looked up again for every reading.
CREATE VIEW match_rank_readings AS
SELECT m.id AS match_id, r.arena_log_file_id AS rank_file_id, r.byte_offset AS rank_byte_offset
FROM matches m
JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
LEFT JOIN arena_log_events f ON f.rowid = (
    SELECT e.rowid FROM arena_log_events e
    WHERE e.match_token = m.token
    ORDER BY e.arena_log_file_id, e.byte_offset
    LIMIT 1)
JOIN arena_rank_observations r ON r.rowid = (
    SELECT o.rowid FROM arena_rank_observations o
    WHERE o.account_id = m.account_id AND o.ladder = 'constructed'
      AND CASE WHEN m.source = 'manual' THEN o.observed_at < m.started_at
          ELSE (o.arena_log_file_id, o.byte_offset) < (f.arena_log_file_id, f.byte_offset)
          END
    ORDER BY o.season DESC, o.arena_log_file_id DESC, o.byte_offset DESC
    LIMIT 1)
WHERE m.match_type = 'ranked';

UPDATE matches SET (rank_file_id, rank_byte_offset) = (
    SELECT v.rank_file_id, v.rank_byte_offset FROM match_rank_readings v
    WHERE v.match_id = matches.id)
WHERE match_type = 'ranked';
