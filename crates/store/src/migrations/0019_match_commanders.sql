-- A commander format match's commanders for both seats (Arena Brawl), from
-- the command zone of game 1; the local seat falls back to the deck's
-- commander. Catalog ids by position. See
-- docs/superpowers/specs/2026-11-12-arena-brawl-design.md section 5.
CREATE TABLE match_commanders (
    match_id INTEGER NOT NULL REFERENCES matches (id) ON DELETE CASCADE,
    seat TEXT NOT NULL CHECK (seat IN ('player', 'opponent')),
    position INTEGER NOT NULL CHECK (position IN (0, 1)),
    catalog_id INTEGER NOT NULL,
    PRIMARY KEY (match_id, seat, position)
) STRICT;
