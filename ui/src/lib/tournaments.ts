import type { TournamentMatchView, TournamentView, WinLossView } from '@/bindings'
import { durationText, gameCell, NO_VALUE } from '@/lib/leagues'

/**
 * Display helpers for the deck Tournaments tab (0.x `TournamentCard`). The
 * backend gives the name, the version span (`v1`, or `v1` to `v3` over several
 * versions) and the counts; these shape them for the card and its rounds table.
 * Durations and game cells read as the league matches table does (`lib/leagues`).
 */

/** One Game 1 to 3 cell: "Win (OTP)"; null for a game not played. */
export type GameCell = ReturnType<typeof gameCell>

/** One rounds table row. */
export type RoundRow = {
  token: string
  /** `R3`; a dash when the round is unknown. */
  round: string
  /** In progress: the Result cell shows a pulsing Live dot. */
  live: boolean
  /** null while live. */
  won: boolean | null
  opponent: string | null
  /** The opponent seat's archetype; null shows "Unknown". */
  archetype: string | null
  /** Always three cells. */
  games: GameCell[]
  time: string
}

/** `R3`, as 0.x labelled rounds. */
export function roundLabel(round: number | null): string {
  return round === null ? NO_VALUE : `R${round}`
}

/** A match still being played has no result yet (0.x showed "Live"). */
export function isLive(match: TournamentMatchView): boolean {
  return match.result === null
}

/** The rounds table, one row per deck match in start order. */
export function roundRows(tournament: TournamentView): RoundRow[] {
  return tournament.matches.map((match) => {
    const live = isLive(match)
    return {
      token: match.token,
      round: roundLabel(match.round),
      live,
      won: live ? null : match.result === 'W',
      opponent: match.opponentName,
      archetype: match.opponentArchetype,
      games: [0, 1, 2].map((index) => gameCell(match.games[index])),
      time: durationText(match.durationSeconds),
    }
  })
}

/** "1 round", "3 rounds". */
export function roundsText(count: number): string {
  return `${count} ${count === 1 ? 'round' : 'rounds'}`
}

/** "2 live". */
export function liveText(count: number): string {
  return `${count} live`
}

/** Games played on one side (0.x showed the total, not the record). */
export function sideTotal(record: WinLossView): number {
  return record.wins + record.losses
}
