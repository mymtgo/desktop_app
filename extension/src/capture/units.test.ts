import { describe, expect, it } from 'vitest'
import type { Frame } from './format'
import { classify, keepUnit, UnitTracker, type HookEvent, type UnitAction } from './units'

function counter() {
  let n = 0
  return () => `u${++n}`
}

function frame(src: Frame['src'], url: string, text = '{"type":"X"}', at = 1): Frame {
  return { at, src, dir: 'in', url, text }
}

const gameOpen = (socketId: string): HookEvent => ({ kind: 'socket_open', socketId, url: '/play/abc' })
const gameClose = (socketId: string): HookEvent => ({ kind: 'socket_close', socketId })
const gameFrame = (socketId: string, text = '{"type":"STATE"}'): HookEvent => ({
  kind: 'frame',
  socketId,
  frame: frame('game_ws', '/play/abc', text),
})
const lobbyFrame = (text = '{"type":"LOBBY"}'): HookEvent => ({
  kind: 'frame',
  frame: frame('lobby_ws', '/ws', text),
})

function appendsTo(actions: UnitAction[], unitId: string) {
  return actions.filter((a) => a.kind === 'append' && a.unitId === unitId)
}

describe('UnitTracker', () => {
  it('game_frames_go_to_their_match_unit', () => {
    const t = new UnitTracker(counter())
    const opened = t.onEvent(1, gameOpen('s1'), 0)
    expect(opened).toEqual([{ kind: 'open', unitId: 'u1', unitKind: 'match', tabId: 1, at: 0 }])
    const actions = t.onEvent(1, gameFrame('s1'), 5)
    expect(actions).toEqual([{ kind: 'append', unitId: 'u1', frame: frame('game_ws', '/play/abc', '{"type":"STATE"}') }])
  })

  it('lobby_frames_between_matches_go_to_a_session_unit', () => {
    const t = new UnitTracker(counter())
    const first = t.onEvent(1, lobbyFrame(), 10)
    expect(first.map((a) => a.kind)).toEqual(['open', 'append'])
    expect(first[0]).toEqual({ kind: 'open', unitId: 'u1', unitKind: 'session', tabId: 1, at: 10 })
    const second = t.onEvent(1, { kind: 'frame', frame: frame('rest', '/api/decks') }, 11)
    expect(second.map((a) => a.kind)).toEqual(['append'])
    expect(appendsTo(second, 'u1')).toHaveLength(1)
  })

  it('a_game_socket_closes_the_session_unit', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, lobbyFrame(), 0)
    const actions = t.onEvent(1, gameOpen('s1'), 1)
    expect(actions).toEqual([
      { kind: 'close', unitId: 'u1' },
      { kind: 'open', unitId: 'u2', unitKind: 'match', tabId: 1, at: 1 },
    ])
  })

  it('lobby and rest frames during a match go to the match unit', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, gameOpen('s1'), 0)
    const actions = t.onEvent(1, lobbyFrame(), 1)
    expect(actions.map((a) => a.kind)).toEqual(['append'])
    expect(appendsTo(actions, 'u1')).toHaveLength(1)
  })

  it('a_reconnect_opens_a_second_match_unit', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, gameOpen('s1'), 0)
    const a1 = t.onEvent(1, gameFrame('s1'), 1)
    expect(t.onEvent(1, gameClose('s1'), 2)).toEqual([{ kind: 'close', unitId: 'u1' }])
    const reopen = t.onEvent(1, gameOpen('s2'), 3)
    expect(reopen).toEqual([{ kind: 'open', unitId: 'u2', unitKind: 'match', tabId: 1, at: 3 }])
    const a2 = t.onEvent(1, gameFrame('s2'), 4)
    expect(appendsTo(a1, 'u1')).toHaveLength(1)
    expect(appendsTo(a2, 'u2')).toHaveLength(1)
    expect(appendsTo(a2, 'u1')).toHaveLength(0)
  })

  it('a_closed_tab_closes_its_units', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, lobbyFrame(), 0)
    t.onEvent(1, gameOpen('s1'), 1)
    t.onEvent(2, lobbyFrame(), 1)
    const actions = t.onTabClosed(1)
    expect(actions).toEqual([{ kind: 'close', unitId: 'u2' }])
    expect(t.onTabClosed(1)).toEqual([])
    expect(t.onTabClosed(2)).toEqual([{ kind: 'close', unitId: 'u3' }])
  })

  it('a_session_unit_closes_after_30_minutes', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, lobbyFrame(), 0)
    const thirty = 30 * 60 * 1000
    expect(t.onEvent(1, lobbyFrame(), thirty - 1).map((a) => a.kind)).toEqual(['append'])
    const actions = t.onEvent(1, lobbyFrame(), thirty)
    expect(actions.map((a) => a.kind)).toEqual(['close', 'open', 'append'])
    expect(actions[0]).toEqual({ kind: 'close', unitId: 'u1' })
    expect(actions[1]).toMatchObject({ kind: 'open', unitId: 'u2', unitKind: 'session', at: thirty })
  })

  it('a full game url opens a match unit', () => {
    const t = new UnitTracker(counter())
    const e: HookEvent = { kind: 'socket_open', socketId: 's1', url: 'wss://endstep.cc/play/forge-b-k-4/ws' }
    expect(t.onEvent(1, e, 0)).toEqual([{ kind: 'open', unitId: 'u1', unitKind: 'match', tabId: 1, at: 0 }])
  })

  it('page_start closes every open unit of the tab only', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, gameOpen('s1'), 0)
    t.onEvent(1, gameOpen('s2'), 0)
    t.onEvent(2, lobbyFrame(), 0)
    expect(t.onEvent(1, { kind: 'page_start' }, 1)).toEqual([
      { kind: 'close', unitId: 'u1' },
      { kind: 'close', unitId: 'u2' },
    ])
    expect(t.onEvent(2, { kind: 'page_start' }, 1)).toEqual([{ kind: 'close', unitId: 'u3' }])
    expect(t.onEvent(1, { kind: 'page_start' }, 2)).toEqual([])
  })

  it('a game frame on an unknown or closed socket goes to the session unit', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, gameOpen('s1'), 0)
    t.onEvent(1, gameClose('s1'), 1)
    const actions = t.onEvent(1, gameFrame('s1'), 2)
    expect(actions.map((a) => a.kind)).toEqual(['open', 'append'])
    expect(actions[0]).toMatchObject({ unitKind: 'session', unitId: 'u2' })
    t.onEvent(1, gameOpen('s3'), 3)
    const stray = t.onEvent(1, gameFrame('nope'), 4)
    expect(appendsTo(stray, 'u3')).toHaveLength(0)
  })

  it('a repeated socket_open is ignored', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, gameOpen('s1'), 0)
    expect(t.onEvent(1, gameOpen('s1'), 1)).toEqual([])
  })

  it('two game sockets at once keep separate units', () => {
    const t = new UnitTracker(counter())
    t.onEvent(1, gameOpen('s1'), 0)
    t.onEvent(1, gameOpen('s2'), 0)
    expect(appendsTo(t.onEvent(1, gameFrame('s1'), 1), 'u1')).toHaveLength(1)
    expect(appendsTo(t.onEvent(1, gameFrame('s2'), 1), 'u2')).toHaveLength(1)
    expect(appendsTo(t.onEvent(1, lobbyFrame(), 1), 'u2')).toHaveLength(1)
    expect(t.onEvent(1, gameClose('s1'), 2)).toEqual([{ kind: 'close', unitId: 'u1' }])
    expect(appendsTo(t.onEvent(1, gameFrame('s2'), 3), 'u2')).toHaveLength(1)
  })

  describe('match over', () => {
    const state = (gameId: string, isMatchOver: boolean) =>
      JSON.stringify({ type: 'GAME_STATE', payload: { gameId, matchScore: { gameNumber: 3, isMatchOver } } })
    const gameOver = (gameId: string) =>
      JSON.stringify({ type: 'GAME_OVER', payload: { gameId, matchScore: { gameNumber: 3, isMatchOver: true } } })
    const delta = (isMatchOver: boolean) =>
      JSON.stringify({ type: 'GAME_DELTA', payload: { patch: { state: { matchScore: { gameNumber: 3, isMatchOver } } } } })

    it('a lobby GAME_OVER for a game of the match closes its unit', () => {
      const t = new UnitTracker(counter())
      t.onEvent(1, gameOpen('s1'), 0)
      t.onEvent(1, gameFrame('s1', state('g3', false)), 1)
      expect(t.onEvent(1, lobbyFrame(gameOver('g3')), 2)).toEqual([
        { kind: 'append', unitId: 'u1', frame: frame('lobby_ws', '/ws', gameOver('g3')) },
        { kind: 'close', unitId: 'u1' },
      ])
      const after = t.onEvent(1, lobbyFrame(), 3)
      expect(after[0]).toMatchObject({ kind: 'open', unitId: 'u2', unitKind: 'session' })
      expect(t.onEvent(1, gameClose('s1'), 4)).toEqual([])
      expect(appendsTo(t.onEvent(1, gameFrame('s1'), 5), 'u2')).toHaveLength(1)
    })

    it('a game state saying the match is over closes the unit of its socket', () => {
      const t = new UnitTracker(counter())
      t.onEvent(1, gameOpen('s1'), 0)
      t.onEvent(1, gameOpen('s2'), 0)
      const actions = t.onEvent(1, gameFrame('s1', state('g3', true)), 1)
      expect(actions.map((a) => a.kind)).toEqual(['append', 'close'])
      expect(actions[1]).toEqual({ kind: 'close', unitId: 'u1' })
      expect(appendsTo(t.onEvent(1, gameFrame('s2'), 2), 'u2')).toHaveLength(1)
    })

    it('a game delta saying the match is over closes the unit of its socket', () => {
      const t = new UnitTracker(counter())
      t.onEvent(1, gameOpen('s1'), 0)
      expect(t.onEvent(1, gameFrame('s1', delta(false)), 1).map((a) => a.kind)).toEqual(['append'])
      expect(t.onEvent(1, gameFrame('s1', delta(true)), 2).map((a) => a.kind)).toEqual(['append', 'close'])
    })

    it('a lobby GAME_OVER for another game leaves the unit open', () => {
      const t = new UnitTracker(counter())
      t.onEvent(1, gameOpen('s1'), 0)
      t.onEvent(1, gameFrame('s1', state('g3', false)), 1)
      expect(t.onEvent(1, lobbyFrame(gameOver('old-game')), 2).map((a) => a.kind)).toEqual(['append'])
      expect(t.onEvent(1, lobbyFrame(state('g3', false)), 3).map((a) => a.kind)).toEqual(['append'])
    })
  })

  it('keepalives_are_dropped', () => {
    const t = new UnitTracker(counter())
    expect(t.onEvent(1, lobbyFrame('{"type":"PING"}'), 0)).toEqual([])
    expect(t.onEvent(1, lobbyFrame('{"type":"PONG"}'), 0)).toEqual([])
    t.onEvent(1, gameOpen('s1'), 0)
    expect(t.onEvent(1, gameFrame('s1', '{"type":"PING"}'), 1)).toEqual([])
    expect(t.onEvent(1, gameFrame('s1', 'not json'), 1).map((a) => a.kind)).toEqual(['append'])
  })
})

const GAME = 'g-1111'

function match(body: Record<string, unknown>, url = '/api/matches/11111111-1111-4111-8111-111111111111', forgeGameId: unknown = GAME): Frame {
  return frame('rest', url, JSON.stringify({ forgeGameId, ...body }))
}

function game(gameId: string = GAME, type = 'GAME_EVENT'): Frame {
  return frame('game_ws', '/play/abc', JSON.stringify({ type, payload: { gameId, n: 1 } }))
}

describe('classify', () => {
  it('a bot participant gives bot', () => {
    const f = match({ participants: [{ isBot: false }, { isAi: true }], myPlayerIndex: 0 })
    expect(classify([game(), f])).toEqual({ classification: 'bot', spectator: false })
  })

  it('human participants give human', () => {
    const f = match({ participants: [{ isBot: false }, {}], myPlayerIndex: 1 })
    expect(classify([f, game(GAME, 'GAME_STATE')])).toEqual({ classification: 'human', spectator: false })
  })

  it('myPlayerIndex null gives spectator', () => {
    const f = match({ participants: [{}, {}], myPlayerIndex: null })
    expect(classify([game(), f])).toEqual({ classification: 'human', spectator: true })
  })

  it('no match rest frame gives unknown', () => {
    const other = frame('rest', '/api/matches/11111111-1111-4111-8111-111111111111/events', '{"participants":[{"isBot":true}]}')
    expect(classify([])).toEqual({ classification: 'unknown', spectator: false })
    expect(classify([game(), other, frame('lobby_ws', '/ws')])).toEqual({ classification: 'unknown', spectator: false })
  })

  it('a rest body at the live matches list never affects classify', () => {
    const live = match({ participants: [{ isBot: true }], myPlayerIndex: 0 }, '/api/matches/live')
    expect(classify([game(), live])).toEqual({ classification: 'unknown', spectator: false })
  })

  it('ignores unparseable frames', () => {
    const bad = frame('rest', '/api/matches/11111111-1111-4111-8111-111111111111', 'nope')
    expect(classify([game(), bad, frame('game_ws', '/play/abc', 'nope')])).toEqual({
      classification: 'unknown',
      spectator: false,
    })
  })

  it('same id with a bot frame gives bot', () => {
    const human = match({ participants: [{}], myPlayerIndex: 0 })
    const bot = match({ participants: [{ isBot: true }], myPlayerIndex: 0 })
    expect(classify([game(), human, bot])).toEqual({ classification: 'bot', spectator: false })
  })

  it('different ids disagreeing gives unknown and not spectator', () => {
    const human = match({ participants: [{}], myPlayerIndex: 0 }, '/api/matches/11111111-1111-4111-8111-111111111111', 'g-a')
    const bot = match({ participants: [{ isBot: true }], myPlayerIndex: null }, '/api/matches/22222222-2222-4222-8222-222222222222', 'g-b')
    expect(classify([game('g-a'), game('g-b'), human, bot])).toEqual({ classification: 'unknown', spectator: false })
  })

  it('empty participants gives unknown', () => {
    expect(classify([game(), match({ participants: [], myPlayerIndex: 0 })])).toEqual({
      classification: 'unknown',
      spectator: false,
    })
  })

  it('a foreign bot match body leaves a human match human and kept', () => {
    const own = match({ participants: [{}, {}], myPlayerIndex: 0 })
    const foreign = match({ participants: [{ isBot: true }], myPlayerIndex: 0 }, '/api/matches/99999999-9999-4999-8999-999999999999', 'g-other')
    const result = classify([game(), own, foreign])
    expect(result).toEqual({ classification: 'human', spectator: false })
    expect(keepUnit('match', result, 3)).toBe(true)
  })

  it('a foreign bot match body alone gives unknown and kept', () => {
    const foreign = match({ participants: [{ isBot: true }], myPlayerIndex: null }, '/api/matches/99999999-9999-4999-8999-999999999999', 'g-other')
    const result = classify([game(), foreign])
    expect(result).toEqual({ classification: 'unknown', spectator: false })
    expect(keepUnit('match', result, 2)).toBe(true)
  })

  it('a match body without forgeGameId is ignored', () => {
    const bot = match({ participants: [{ isBot: true }], myPlayerIndex: 0 }, '/api/matches/11111111-1111-4111-8111-111111111111', null)
    expect(classify([game(), bot])).toEqual({ classification: 'unknown', spectator: false })
  })

  it('no game frames gives unknown', () => {
    const bot = match({ participants: [{ isBot: true }], myPlayerIndex: null })
    expect(classify([bot])).toEqual({ classification: 'unknown', spectator: false })
    const lobbyWithId = frame('lobby_ws', '/ws', JSON.stringify({ type: 'GAME_EVENT', payload: { gameId: GAME } }))
    expect(classify([lobbyWithId, bot])).toEqual({ classification: 'unknown', spectator: false })
  })
})

describe('keepUnit', () => {
  const human = { classification: 'human' as const, spectator: false }
  it('always drops bots', () => {
    const bot = { classification: 'bot' as const, spectator: false }
    expect(keepUnit('match', bot, 5)).toBe(false)
  })
  it('keeps unknown', () => {
    expect(keepUnit('match', { classification: 'unknown', spectator: false }, 5)).toBe(true)
  })
  it('drops spectators', () => {
    expect(keepUnit('match', { ...human, spectator: true }, 5)).toBe(false)
  })
  it('drops empty session units, keeps non-empty', () => {
    const unknown = { classification: 'unknown' as const, spectator: false }
    expect(keepUnit('session', unknown, 0)).toBe(false)
    expect(keepUnit('session', unknown, 3)).toBe(true)
  })
  it('session units ignore bot and spectator', () => {
    expect(keepUnit('session', { classification: 'bot', spectator: true }, 2)).toBe(true)
  })
})
