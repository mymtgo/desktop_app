-- The offline archetype download (2026-10-27 offline download design).
--
-- archetypes.offline_held: 1 for a server archetype the player downloaded
-- while offline, held as a Custom (manual) row they own. Going back online
-- turns every marked row back into a downloaded one. 0 for every other row,
-- including the player's own Custom archetypes.
ALTER TABLE archetypes ADD COLUMN offline_held INTEGER NOT NULL DEFAULT 0
    CHECK (offline_held IN (0, 1));
