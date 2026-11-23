-- League run identity (server spec 2026-10-23): a match report carries its
-- league's sync client id (`league_client_id`), and the server takes the
-- last report it is sent. The body is packed when the report is queued, so
-- when the id a match's report named changes, the queued or sent report is
-- dropped and the next enqueue builds a fresh one (as an unreadable report
-- is, outbox.rs `mark_unreadable`).
--
-- The id changes when the league's own id moves (`take_synced_id` in a
-- merge, an import or a pull filling or replacing it, a first token) or when
-- the match moves to another league or is given one after its report went.
--
-- Only this device's reports go: a pending or claimed row, or a sent row it
-- attempted. A `sent` row with no attempt was recorded for a match another
-- device or 0.x reported (`record_sent`); this device does not report it.
-- A deleted match is not reported again.
CREATE TRIGGER leagues_client_id_changed_requeues_reports
AFTER UPDATE OF sync_client_id ON leagues
WHEN OLD.sync_client_id IS NOT NEW.sync_client_id
BEGIN
    DELETE FROM outbox
    WHERE kind = 'match_report'
      AND (status <> 'sent' OR attempts > 0)
      AND subject IN (SELECT token FROM matches
                      WHERE league_id = NEW.id AND hidden_at IS NULL);
END;

CREATE TRIGGER matches_league_changed_requeues_report
AFTER UPDATE OF league_id ON matches
WHEN OLD.league_id IS NOT NEW.league_id AND NEW.hidden_at IS NULL
BEGIN
    DELETE FROM outbox
    WHERE kind = 'match_report' AND subject = NEW.token
      AND (status <> 'sent' OR attempts > 0);
END;
