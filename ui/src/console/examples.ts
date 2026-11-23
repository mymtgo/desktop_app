/** The example queries in the Schema pane: the `v_*` views only. */
export const examples: { name: string; sql: string }[] = [
  {
    name: 'Win rate by format',
    sql: `SELECT format,
       count(*)                                AS matches,
       sum(outcome = 'win')                    AS wins,
       round(avg(outcome = 'win') * 100, 1)    AS win_pct
FROM v_matches
WHERE outcome IN ('win', 'loss', 'draw')
GROUP BY format
ORDER BY matches DESC`,
  },
  {
    name: 'Win rate on the play and on the draw',
    sql: `SELECT CASE on_play WHEN 1 THEN 'play' ELSE 'draw' END AS side,
       count(*)                                AS games,
       round(avg(result = 'win') * 100, 1)     AS win_pct
FROM v_games
WHERE on_play IS NOT NULL AND result IN ('win', 'loss', 'draw')
GROUP BY on_play
ORDER BY on_play DESC`,
  },
  {
    name: 'Results by opponent archetype',
    sql: `SELECT coalesce(opponent_archetype, 'unknown')  AS opponent_archetype,
       count(*)                                AS matches,
       sum(outcome = 'win')                    AS wins,
       sum(outcome = 'loss')                   AS losses,
       round(avg(outcome = 'win') * 100, 1)    AS win_pct
FROM v_matches
WHERE outcome IN ('win', 'loss', 'draw')
GROUP BY 1
ORDER BY matches DESC`,
  },
  {
    name: 'Matches in one format',
    sql: `SELECT started_at, deck_name, archetype, opponent_archetype, outcome, games_won, games_lost
FROM v_matches
WHERE format = :format
ORDER BY started_ms DESC`,
  },
  {
    name: 'Mulligans by deck',
    sql: `SELECT m.deck_name,
       count(*)                                AS games,
       sum(g.mulligans > 0)                    AS mulliganed,
       round(avg(g.mulligans), 2)              AS avg_mulligans,
       round(avg(CASE WHEN g.mulligans > 0 THEN g.result = 'win' END) * 100, 1) AS mulligan_win_pct
FROM v_games g
JOIN v_matches m ON m.match_id = g.match_id
WHERE m.deck_name IS NOT NULL
GROUP BY m.deck_name
ORDER BY games DESC`,
  },
  {
    name: 'Most cast cards',
    sql: `SELECT card_name,
       sum(casts)                              AS casts,
       count(*)                                AS games
FROM v_card_games
WHERE side = 'me'
GROUP BY card_name
ORDER BY casts DESC
LIMIT 50`,
  },
  {
    name: 'Deck records',
    sql: `SELECT name, format, archetype, matches, wins, losses,
       round(wins * 100.0 / nullif(wins + losses, 0), 1) AS win_pct
FROM v_decks
WHERE archived = 0
ORDER BY matches DESC`,
  },
  {
    name: 'Compare card stats between two archetypes',
    sql: `SELECT card_name,
       archetype,
       count(*)                                  AS games,
       round(avg(won) * 100, 1)                  AS win_pct,
       round(avg(CASE WHEN in_starting_hand > 0 THEN won END) * 100, 1) AS kept_win_pct,
       round(avg(CASE WHEN seen > 0 THEN won END) * 100, 1)             AS seen_win_pct
FROM v_card_games
WHERE side = 'me' AND archetype IN (:first_archetype, :second_archetype)
GROUP BY card_name, archetype
ORDER BY card_name, archetype`,
  },
  {
    name: 'Compare card stats between two decks',
    sql: `SELECT card_name,
       deck_name,
       count(*)                                  AS games,
       round(avg(won) * 100, 1)                  AS win_pct,
       round(avg(CASE WHEN in_starting_hand > 0 THEN won END) * 100, 1) AS kept_win_pct,
       round(avg(CASE WHEN seen > 0 THEN won END) * 100, 1)             AS seen_win_pct
FROM v_card_games
WHERE side = 'me' AND deck_name IN (:first_deck, :second_deck)
GROUP BY card_name, deck_name
ORDER BY card_name, deck_name`,
  },
]
