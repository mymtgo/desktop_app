-- League entries (#757): each run's length as its league's panel states it
-- (NULL: none seen, so the kind's default), and the partial index the
-- leagues pages walk covers join requests, which the fold reads as a new
-- entry of the league whose panel is logged with them.
ALTER TABLE leagues ADD COLUMN rounds INTEGER CHECK (rounds IS NULL OR rounds > 0);
DROP INDEX log_events_leagues;
CREATE INDEX log_events_leagues ON log_events (log_file_id, byte_offset)
    WHERE kind IN ('league_joined', 'league_dropped', 'league_join_request');
