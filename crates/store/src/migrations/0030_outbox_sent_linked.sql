-- Whether a sent row went out signed in (1) or in device mode (0). The API
-- ties a match report to the account only when it carries the account's
-- bearer, so a report sent while signed out stays unowned there. Once the
-- device is signed in again, its reports sent in device mode are dropped
-- from the outbox and the next enqueue sends them again with the bearer
-- (`OutboxEdit::RequeueDeviceReports`).
--
-- The reports this device sent before this migration are marked 1: which
-- of them went signed out is not known, and sending a whole history again
-- is too much. A gap from before it is fixed on the API by hand. A `sent`
-- row with no attempt was recorded for a match another device or 0.x
-- reported (`record_sent`) and stays NULL.
ALTER TABLE outbox ADD COLUMN sent_linked INTEGER CHECK (sent_linked IN (0, 1));

UPDATE outbox SET sent_linked = 1
WHERE kind = 'match_report' AND status = 'sent' AND attempts > 0;
