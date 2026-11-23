import type { Frame } from './format'
import { stripUrl } from './strip'

export type HookEvent =
  | { kind: 'socket_open'; socketId: string; url: string }
  | { kind: 'socket_close'; socketId: string }
  | { kind: 'page_start' }
  | { kind: 'frame'; socketId?: string; frame: Frame }

export type UnitAction =
  | { kind: 'open'; unitId: string; unitKind: 'match' | 'session'; tabId: number; at: number }
  | { kind: 'append'; unitId: string; frame: Frame }
  | { kind: 'close'; unitId: string }

export type Classification = 'human' | 'bot' | 'unknown'
export interface ClassifyResult {
  classification: Classification
  spectator: boolean
}

const SESSION_MAX_AGE_MS = 30 * 60 * 1000
const GAME_PATH_PREFIX = '/play/'
const MATCH_REST_URL = /^\/api\/matches\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/

interface MatchUnit {
  socketId: string
  unitId: string
  // Games this unit's game socket carried.
  gameIds: Set<string>
}

interface TabState {
  session: { unitId: string; openedAt: number } | null
  // Open match units, oldest first.
  matches: MatchUnit[]
}

function isKeepalive(frame: Frame): boolean {
  if (frame.src === 'rest') return false
  try {
    const body: unknown = JSON.parse(frame.text)
    if (typeof body !== 'object' || body === null) return false
    const type = (body as { type?: unknown }).type
    return type === 'PING' || type === 'PONG'
  } catch {
    return false
  }
}

export class UnitTracker {
  private tabs = new Map<number, TabState>()

  constructor(private newId: () => string) {}

  onEvent(tabId: number, event: HookEvent, now: number): UnitAction[] {
    const actions: UnitAction[] = []
    const tab = this.tabState(tabId)

    if (tab.session && now - tab.session.openedAt >= SESSION_MAX_AGE_MS) {
      actions.push({ kind: 'close', unitId: tab.session.unitId })
      tab.session = null
    }

    if (event.kind === 'page_start') {
      this.tabs.delete(tabId)
      if (tab.session) actions.push({ kind: 'close', unitId: tab.session.unitId })
      for (const m of tab.matches) actions.push({ kind: 'close', unitId: m.unitId })
    } else if (event.kind === 'socket_open') {
      const alreadyOpen = tab.matches.some((m) => m.socketId === event.socketId)
      if (!alreadyOpen && stripUrl(event.url).startsWith(GAME_PATH_PREFIX)) {
        if (tab.session) {
          actions.push({ kind: 'close', unitId: tab.session.unitId })
          tab.session = null
        }
        const unitId = this.newId()
        tab.matches.push({ socketId: event.socketId, unitId, gameIds: new Set() })
        actions.push({ kind: 'open', unitId, unitKind: 'match', tabId, at: now })
      }
    } else if (event.kind === 'socket_close') {
      const index = tab.matches.findIndex((m) => m.socketId === event.socketId)
      if (index >= 0) {
        const [closed] = tab.matches.splice(index, 1)
        actions.push({ kind: 'close', unitId: closed.unitId })
      }
    } else if (!isKeepalive(event.frame)) {
      this.routeFrame(tabId, tab, event, now, actions)
    }

    return actions
  }

  onTabClosed(tabId: number): UnitAction[] {
    const tab = this.tabs.get(tabId)
    if (!tab) return []
    this.tabs.delete(tabId)
    const actions: UnitAction[] = []
    if (tab.session) actions.push({ kind: 'close', unitId: tab.session.unitId })
    for (const m of tab.matches) actions.push({ kind: 'close', unitId: m.unitId })
    return actions
  }

  private tabState(tabId: number): TabState {
    let tab = this.tabs.get(tabId)
    if (!tab) {
      tab = { session: null, matches: [] }
      this.tabs.set(tabId, tab)
    }
    return tab
  }

  private routeFrame(
    tabId: number,
    tab: TabState,
    event: Extract<HookEvent, { kind: 'frame' }>,
    now: number,
    actions: UnitAction[],
  ) {
    const { frame } = event
    const payload = frame.src === 'rest' ? null : payloadOf(frame)
    let match: MatchUnit | undefined

    if (frame.src === 'game_ws') {
      // A game frame whose socket is unknown or closed is not part of any match unit.
      match = tab.matches.find((m) => m.socketId === event.socketId)
      if (match && typeof payload?.gameId === 'string') match.gameIds.add(payload.gameId)
    } else if (tab.matches.length > 0) {
      match = tab.matches[tab.matches.length - 1]
    }
    if (match) {
      actions.push({ kind: 'append', unitId: match.unitId, frame })
      // Endstep can keep the game socket open after the match ends, so the end closes the unit.
      if (payload !== null && endsMatch(frame.src, payload, match)) {
        tab.matches.splice(tab.matches.indexOf(match), 1)
        actions.push({ kind: 'close', unitId: match.unitId })
      }
      return
    }
    if (!tab.session) {
      tab.session = { unitId: this.newId(), openedAt: now }
      actions.push({ kind: 'open', unitId: tab.session.unitId, unitKind: 'session', tabId, at: now })
    }
    actions.push({ kind: 'append', unitId: tab.session.unitId, frame })
  }
}

function payloadOf(frame: Frame): Record<string, unknown> | null {
  try {
    const body: unknown = JSON.parse(frame.text)
    if (typeof body !== 'object' || body === null) return null
    const payload = (body as { payload?: unknown }).payload
    return typeof payload === 'object' && payload !== null ? (payload as Record<string, unknown>) : null
  } catch {
    return null
  }
}

// The `payload.gameId` of a game socket frame, if it has one.
function gameIdOf(frame: Frame): string | null {
  if (frame.src !== 'game_ws') return null
  const gameId = payloadOf(frame)?.gameId
  return typeof gameId === 'string' ? gameId : null
}

function saysMatchOver(holder: unknown): boolean {
  if (typeof holder !== 'object' || holder === null) return false
  const score = (holder as { matchScore?: unknown }).matchScore
  if (typeof score !== 'object' || score === null) return false
  return (score as { isMatchOver?: unknown }).isMatchOver === true
}

// Whether a frame's payload says `match` is over: a state or delta on its own game socket, or a
// lobby frame naming a game that socket carried (the lobby also reports other games).
function endsMatch(src: Frame['src'], payload: Record<string, unknown>, match: MatchUnit): boolean {
  const patch = payload.patch
  const deltaState = typeof patch === 'object' && patch !== null ? (patch as { state?: unknown }).state : undefined
  if (!saysMatchOver(payload) && !saysMatchOver(deltaState)) return false
  if (src === 'game_ws') return true
  return typeof payload.gameId === 'string' && match.gameIds.has(payload.gameId)
}

export function classify(lines: Frame[]): ClassifyResult {
  // A match body counts only when its forgeGameId is a game this unit's game socket carried:
  // the page can fetch other matches (a lobby list, a previous match) while this one runs.
  const gameIds = new Set<string>()
  for (const frame of lines) {
    const gameId = gameIdOf(frame)
    if (gameId !== null) gameIds.add(gameId)
  }

  // Per match id: any bot wins, spectator if any frame has myPlayerIndex null.
  const matches = new Map<string, { bot: boolean; spectator: boolean }>()

  for (const frame of lines) {
    if (frame.src !== 'rest') continue
    const found = MATCH_REST_URL.exec(frame.url)
    if (!found) continue
    let body: unknown
    try {
      body = JSON.parse(frame.text)
    } catch {
      continue
    }
    if (typeof body !== 'object' || body === null) continue
    const { participants, myPlayerIndex, forgeGameId } = body as {
      participants?: unknown
      myPlayerIndex?: unknown
      forgeGameId?: unknown
    }
    if (typeof forgeGameId !== 'string' || !gameIds.has(forgeGameId)) continue
    if (!Array.isArray(participants) || participants.length === 0) continue
    const entry = matches.get(found[1]) ?? { bot: false, spectator: false }
    for (const p of participants) {
      if (typeof p === 'object' && p !== null) {
        const { isBot, isAi } = p as { isBot?: unknown; isAi?: unknown }
        if (isBot === true || isAi === true) entry.bot = true
      }
    }
    if (myPlayerIndex === null) entry.spectator = true
    matches.set(found[1], entry)
  }

  const results = [...matches.values()]
  if (results.length === 0) return { classification: 'unknown', spectator: false }
  const first = results[0]
  if (results.some((r) => r.bot !== first.bot || r.spectator !== first.spectator)) {
    return { classification: 'unknown', spectator: false }
  }
  return { classification: first.bot ? 'bot' : 'human', spectator: first.spectator }
}

export function keepUnit(
  kind: 'match' | 'session',
  result: ClassifyResult,
  frameCount: number,
): boolean {
  if (kind === 'session') return frameCount > 0
  if (result.spectator) return false
  if (result.classification === 'bot') return false
  return true
}
