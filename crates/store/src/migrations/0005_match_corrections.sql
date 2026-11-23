-- Match corrections (edit a match spec, sections 2 and 7; #393).
--
-- matches.corrected_at: the time of the player's last fix of the match
-- (opponent, start, end, outcome and games, written into the real columns
-- by the edit). NULL for a match never corrected. While it is set, the
-- projection's upsert and game writes and the 0.x and pulled import leave
-- the row and its games alone, so a rebuild or a re-import never undoes the
-- fix, and a removed match with a fix is detached, not deleted.
ALTER TABLE matches ADD COLUMN corrected_at INTEGER;
