-- A pulled row this device cannot apply yet (#830): an Arena run with no
-- Arena account here, or an Arena or Endstep match that came without its
-- source. parked_hash is the server's hash of the copy that was refused,
-- parked_reason why. The run sends the row's client id as known, so the
-- server offers it again only once its copy changes; a run parked for
-- want of an account is fetched again as soon as one exists. A row that is
-- applied later clears both.
ALTER TABLE synced_rows ADD COLUMN parked_hash TEXT;
ALTER TABLE synced_rows ADD COLUMN parked_reason TEXT
    CHECK (parked_reason IN ('no_arena_account', 'no_sources'));
