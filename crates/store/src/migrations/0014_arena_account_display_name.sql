-- An Arena account known only by its user id (a fast login whose screen name
-- has no #12345 tag, trap 29): its username is 'arena:' followed by the user
-- id, so it never names an MTGO account, and its untagged screen name is
-- accounts.display_name. NULL for every other account (the username shows).
ALTER TABLE accounts ADD COLUMN display_name TEXT;
