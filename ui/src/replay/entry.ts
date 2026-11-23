import type { ReplayPreviousSideboard } from '@/bindings'
import { gameSideboard, type ReplaySideboardBaseline } from './replaySideboard'

/**
 * The replay window's entry (replays spec section 6): the window opens
 * `replay.html?game={id}` and the page shows that game; the app pushes
 * `replay-game-changed` when another game is opened, and the game picker
 * switches games the same way, in place.
 */

/** The game row id named by a location's query (`?game=12`), or null. */
export function replayGameId(search: string): number | null {
  const value = new URLSearchParams(search).get('game')
  if (value === null || !/^\d+$/.test(value)) return null
  const id = Number(value)
  return id > 0 && Number.isSafeInteger(id) ? id : null
}

/**
 * The game to show after a switch: the new one, or null when it is already
 * shown. An Arena or Endstep record shows another game of its match in
 * place when the in-window picker is used (`moved`), so `shown` is then not
 * what the window shows, and asking for it again opens it.
 */
export function switchedGame(shown: number | null, next: number, moved = false): number | null {
  return shown === next && !moved ? null : next
}

/**
 * The game to show after asking the app for the newest request (the page asks
 * once it is listening, because a click during its load pushed an event
 * nobody heard): that request, or null for none or the game already shown.
 */
export function requestedGame(shown: number | null, requested: number | null): number | null {
  return requested === null ? null : switchedGame(shown, requested)
}

/**
 * The viewer's `previousSideboard` from `replay_view`'s: the earlier game's
 * entries as cards, one per copy (0.x `games/Show.vue`).
 */
export function previousBaseline(previous: ReplayPreviousSideboard | null): ReplaySideboardBaseline | null {
  return previous ? { game: previous.game, cards: gameSideboard([], previous.sideboard) ?? [] } : null
}
