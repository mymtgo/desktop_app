-- Arena format codes (ARENA_HISTORIC) now key like the server's (historic):
-- see docs/superpowers/specs/2026-11-11-arena-match-reports-design.md
-- section 13. Rekey the stored archetype rows written under the old key.
-- Match and deck format codes are raw codes and do not change.
UPDATE archetypes SET format = substr(format, 7) WHERE format LIKE 'arena\_%' ESCAPE '\';
