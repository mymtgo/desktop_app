import { describe, expect, it } from 'vitest'
import { frameLine, headerLine, type Frame } from '../capture/format'
import {
  framesFromLines,
  recoverOnStart,
  sendClosedUnits,
  splitIntoParts,
  type DeliveryLimits,
  type DropRecord,
  type HostReply,
} from './delivery'
import { MemoryStore } from './store'

const U1 = '11111111-1111-4111-8111-111111111111'
const settings = { extension: '0.1.0', browser: 'chrome' }

const GAME = 'g-1111'
const noDrop = async () => {}

function gameFrame(at: number, text = JSON.stringify({ type: 'GAME_EVENT', payload: { gameId: GAME } })): Frame {
  return { at, src: 'game_ws', dir: 'in', url: '/play/abc/ws', text }
}

function matchFrame(at: number, bot: boolean): Frame {
  const body = {
    forgeGameId: GAME,
    participants: [{ name: 'Player' }, { name: 'Opponent', isBot: bot }],
    myPlayerIndex: 0,
  }
  return { at, src: 'rest', dir: 'in', url: '/api/matches/11111111-1111-4111-8111-111111111111', text: JSON.stringify(body) }
}

async function storeUnit(
  store: MemoryStore,
  frames: Frame[],
  options: { unitKind?: 'match' | 'session'; close?: boolean } = {},
): Promise<number> {
  const counter = await store.open({ unitId: U1, unitKind: options.unitKind ?? 'match', tabId: 7, startedAt: 1000 })
  for (const frame of frames) await store.append(U1, frameLine(frame))
  if (options.close ?? true) await store.markClosed(U1)
  return counter
}

function recorder(reply: HostReply | Error) {
  const sent: { type: string; unit_id: string; content: string }[] = []
  const send = async (msg: object): Promise<HostReply> => {
    sent.push(msg as { type: string; unit_id: string; content: string })
    if (reply instanceof Error) throw reply
    return reply
  }
  return { sent, send }
}

// A transport for tests of the one-message path: the parts path must not be used.
function only(send: (msg: object) => Promise<HostReply>) {
  return {
    send,
    sendParts: async (): Promise<HostReply> => {
      throw new Error('sendParts was not expected')
    },
  }
}

describe('sendClosedUnits', () => {
  it('a_delivered_unit_is_removed', async () => {
    const store = new MemoryStore()
    const frames = [matchFrame(1001, false), gameFrame(1002), gameFrame(1003)]
    const counter = await storeUnit(store, frames)
    const { sent, send } = recorder({ ok: true })

    const report = await sendClosedUnits(store, only(send), settings, noDrop)

    expect(sent).toHaveLength(1)
    expect(sent[0].type).toBe('deliver')
    expect(sent[0].unit_id).toBe(U1)
    const lines = sent[0].content.split('\n')
    expect(lines[0]).toBe(
      headerLine({
        mymtgo_endstep_capture: 1,
        unit_id: U1,
        kind: 'match',
        started_at: 1000,
        counter,
        extension: '0.1.0',
        browser: 'chrome',
        developer: false,
        classification: 'human',
      }),
    )
    expect(lines.slice(1)).toEqual(frames.map(frameLine))
    expect(await store.closedUnits()).toEqual([])
    expect(await store.lines(U1)).toEqual([])
    expect(report.sent).toEqual([U1])
  })

  it('a_failed_delivery_keeps_the_unit', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [gameFrame(1001)])

    const refused = recorder({ ok: false, error: 'x' })
    await sendClosedUnits(store, only(refused.send), settings, noDrop)
    let units = await store.closedUnits()
    expect(units).toHaveLength(1)
    expect(units[0].error).toBe('x')

    const thrown = recorder(new Error('Specified native messaging host not found.'))
    const report = await sendClosedUnits(store, only(thrown.send), settings, noDrop)
    units = await store.closedUnits()
    expect(units).toHaveLength(1)
    expect(units[0].error).toBe('Specified native messaging host not found.')
    expect(report.waiting).toEqual([U1])

    const working = recorder({ ok: true })
    await sendClosedUnits(store, only(working.send), settings, noDrop)
    expect(working.sent).toHaveLength(1)
    expect(await store.closedUnits()).toEqual([])
  })

  it('open_units_are_closed_and_sent_on_start', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [gameFrame(1001)], { close: false })
    const { sent, send } = recorder({ ok: true })

    await sendClosedUnits(store, only(send), settings, noDrop)
    expect(sent).toHaveLength(0)

    await recoverOnStart(store)
    expect(await store.openUnits()).toEqual([])
    await sendClosedUnits(store, only(send), settings, noDrop)
    expect(sent.map((m) => m.unit_id)).toEqual([U1])
    expect(await store.closedUnits()).toEqual([])
  })

  it('bot_units_are_always_dropped', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, true), gameFrame(1002)])
    const { sent, send } = recorder({ ok: true })
    const report = await sendClosedUnits(store, only(send), settings, noDrop)
    expect(sent).toHaveLength(0)
    expect(report.dropped).toEqual([U1])
    expect(await store.closedUnits()).toEqual([])
  })

  it('human_units_are_not_developer', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false)])
    const { sent, send } = recorder({ ok: true })
    await sendClosedUnits(store, only(send), settings, noDrop)
    expect(JSON.parse(sent[0].content.split('\n')[0]).developer).toBe(false)
  })

  it('unknown_units_are_sent', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [gameFrame(1001), gameFrame(1002)])
    const { sent, send } = recorder({ ok: true })
    await sendClosedUnits(store, only(send), settings, noDrop)
    expect(sent).toHaveLength(1)
    const header = JSON.parse(sent[0].content.split('\n')[0])
    expect(header.classification).toBe('unknown')
    expect(header.developer).toBe(false)
  })

  it('session_units_are_unknown_and_empty_ones_dropped', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false)], { unitKind: 'session' })
    const { sent, send } = recorder({ ok: true })
    await sendClosedUnits(store, only(send), settings, noDrop)
    const header = JSON.parse(sent[0].content.split('\n')[0])
    expect(header.kind).toBe('session')
    expect(header.classification).toBe('unknown')

    const empty = new MemoryStore()
    await storeUnit(empty, [], { unitKind: 'session' })
    const second = recorder({ ok: true })
    const report = await sendClosedUnits(empty, only(second.send), settings, noDrop)
    expect(second.sent).toHaveLength(0)
    expect(report.dropped).toEqual([U1])
  })

  it('dropped_units_are_recorded_with_their_reason', async () => {
    const run = async (frames: Frame[], unitKind: 'match' | 'session' = 'match') => {
      const store = new MemoryStore()
      await storeUnit(store, frames, { unitKind })
      const drops: DropRecord[] = []
      const { send } = recorder({ ok: true })
      await sendClosedUnits(store, only(send), settings, async (drop) => {
        drops.push(drop)
      })
      return drops
    }
    const spectatorBody = { forgeGameId: GAME, participants: [{}, {}], myPlayerIndex: null }
    const spectator: Frame = { at: 1001, src: 'rest', dir: 'in', url: '/api/matches/11111111-1111-4111-8111-111111111111', text: JSON.stringify(spectatorBody) }

    expect(await run([gameFrame(1000), matchFrame(1001, true)])).toEqual([{ kind: 'match', reason: 'bot' }])
    expect(await run([gameFrame(1000), spectator])).toEqual([{ kind: 'match', reason: 'spectator' }])
    expect(await run([], 'session')).toEqual([{ kind: 'session', reason: 'empty' }])
    expect(await run([gameFrame(1000), matchFrame(1001, false)])).toEqual([])
    expect(await run([gameFrame(1000)], 'session')).toEqual([])
  })

  it('a_failing_drop_recorder_still_drops_the_unit', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [gameFrame(1000), matchFrame(1001, true)])
    const { send } = recorder({ ok: true })
    const report = await sendClosedUnits(store, only(send), settings, async () => {
      throw new Error('storage full')
    })
    expect(report.dropped).toEqual([U1])
    expect(await store.closedUnits()).toEqual([])
  })

  it('secrets_are_never_stored_and_bots_still_classify_from_stored_lines', async () => {
    const store = new MemoryStore()
    const attach: Frame = {
      at: 1000,
      src: 'game_ws',
      dir: 'out',
      url: '/play/abc/ws',
      text: JSON.stringify({ type: 'ATTACH', payload: { ticket: 'v1.SECRET-TICKET', deck: 1 } }),
    }
    await storeUnit(store, [attach, matchFrame(1001, false), gameFrame(1002)])

    // Everything the store holds, private fields included.
    expect(JSON.stringify(store)).not.toContain('SECRET-TICKET')
    expect(JSON.stringify(store)).toContain('ATTACH')
    expect(framesFromLines(await store.lines(U1))[0].text).toBe('{"type":"ATTACH","payload":{"deck":1}}')

    const { sent, send } = recorder({ ok: true })
    await sendClosedUnits(store, only(send), settings, noDrop)
    expect(sent[0].content).not.toContain('SECRET-TICKET')
    expect(JSON.parse(sent[0].content.split('\n')[0]).classification).toBe('human')
  })

  it('counters_increase_per_open', async () => {
    const store = new MemoryStore()
    const a = await store.open({ unitId: 'a', unitKind: 'match', tabId: 1, startedAt: 1 })
    const b = await store.open({ unitId: 'b', unitKind: 'session', tabId: 1, startedAt: 2 })
    expect(b).toBe(a + 1)
  })
})

describe('splitIntoParts', () => {
  const bytes = (text: string) => new TextEncoder().encode(text).byteLength

  it('the_parts_joined_equal_the_joined_lines', () => {
    const lines = ['header', 'a'.repeat(10), 'b'.repeat(25), 'c', '', 'e'.repeat(7), '\u{1F600}x']
    for (const max of [1, 8, 12, 30, 1000]) {
      expect(splitIntoParts(lines, max).join('')).toBe(lines.join('\n'))
    }
  })

  it('no_part_is_over_the_limit_unless_one_line_is', () => {
    const lines = ['h', 'aaaa', 'bbbb', 'cc', 'dddddddddddddddd', 'e']
    const parts = splitIntoParts(lines, 10)
    expect(parts.length).toBeGreaterThan(1)
    for (const part of parts) {
      if (bytes(part) > 10) expect(part.trimEnd().split('\n')).toHaveLength(1)
    }
    expect(parts).toContain('dddddddddddddddd\n')
  })

  it('only_the_last_part_has_no_trailing_newline', () => {
    const parts = splitIntoParts(['h', 'aaaa', 'bbbb', 'cccc'], 8)
    for (const part of parts.slice(0, -1)) expect(part.endsWith('\n')).toBe(true)
    expect(parts[parts.length - 1].endsWith('\n')).toBe(false)
  })

  it('a_one_line_unit_is_one_part', () => {
    expect(splitIntoParts(['header'], 1)).toEqual(['header'])
  })
})

describe('sendClosedUnits in parts', () => {
  const small: DeliveryLimits = { maxContentBytes: 200, maxCaptureBytes: 100_000, partBytes: 150, maxLineBytes: 100_000 }

  function partsRecorder(reply: (messages: object[]) => HostReply) {
    const sentParts: object[][] = []
    const sent: object[] = []
    return {
      sentParts,
      sent,
      transport: {
        send: async (msg: object): Promise<HostReply> => {
          sent.push(msg)
          return { ok: true }
        },
        sendParts: async (messages: Iterable<object>): Promise<HostReply> => {
          const all = [...messages]
          sentParts.push(all)
          return reply(all)
        },
      },
    }
  }

  const bigFrames = () => Array.from({ length: 8 }, (_, n) => gameFrame(1002 + n))

  it('a_big_unit_goes_in_parts_and_is_removed_on_ok', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false), ...bigFrames()])
    const { sent, sentParts, transport } = partsRecorder(() => ({ ok: true }))

    const report = await sendClosedUnits(store, transport, settings, noDrop, small)

    expect(sent).toEqual([])
    expect(sentParts).toHaveLength(1)
    const messages = sentParts[0] as { type: string; unit_id: string; index?: number; content?: string; parts?: number }[]
    const partMessages = messages.slice(0, -1)
    expect(partMessages.length).toBeGreaterThan(1)
    expect(partMessages.map((m) => m.type)).toEqual(partMessages.map(() => 'deliver_part'))
    expect(partMessages.map((m) => m.index)).toEqual(partMessages.map((_, i) => i))
    expect(messages[messages.length - 1]).toEqual({ type: 'deliver_end', unit_id: U1, parts: partMessages.length })
    const joined = partMessages.map((m) => m.content).join('')
    expect(joined.split('\n').slice(1)).toEqual([matchFrame(1001, false), ...bigFrames()].map(frameLine))
    expect(await store.closedUnits()).toEqual([])
    expect(report.sent).toEqual([U1])
  })

  it('a_failed_part_keeps_the_unit_waiting_with_the_error', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false), ...bigFrames()])
    const { transport } = partsRecorder(() => ({ ok: false, error: 'part out of order' }))

    const report = await sendClosedUnits(store, transport, settings, noDrop, small)

    expect(report.waiting).toEqual([U1])
    expect((await store.closedUnits())[0].error).toBe('part out of order')
  })

  it('an_old_helper_gets_a_clear_waiting_error', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false), ...bigFrames()])
    const { transport } = partsRecorder(() => ({ ok: false, error: 'bad request' }))

    await sendClosedUnits(store, transport, settings, noDrop, small)

    expect((await store.closedUnits())[0].error).toBe('too large for this MyMTGO version: update MyMTGO')
  })

  it('a_port_that_throws_keeps_the_unit_waiting', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false), ...bigFrames()])
    const { transport } = partsRecorder(() => {
      throw new Error('the helper closed the connection')
    })

    const report = await sendClosedUnits(store, transport, settings, noDrop, small)

    expect(report.waiting).toEqual([U1])
    expect((await store.closedUnits())[0].error).toBe('the helper closed the connection')
  })

  it('a_unit_over_the_capture_cap_waits_as_too_large', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false), ...bigFrames()])
    const { sent, sentParts, transport } = partsRecorder(() => ({ ok: true }))

    await sendClosedUnits(store, transport, settings, noDrop, { ...small, maxCaptureBytes: 300 })

    expect(sent).toEqual([])
    expect(sentParts).toEqual([])
    expect((await store.closedUnits())[0].error).toBe('too large')
  })

  it('a_line_over_the_line_limit_waits_as_too_large_without_sending', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false), ...bigFrames()])
    const { sent, sentParts, transport } = partsRecorder(() => ({ ok: true }))

    await sendClosedUnits(store, transport, settings, noDrop, { ...small, maxLineBytes: 50 })

    expect(sent).toEqual([])
    expect(sentParts).toEqual([])
    expect((await store.closedUnits())[0].error).toBe('too large')
  })

  it('a_long_line_is_fine_when_the_unit_goes_in_one_message', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false)])
    const { sent, transport } = partsRecorder(() => ({ ok: true }))

    await sendClosedUnits(store, transport, settings, noDrop, { ...small, maxContentBytes: 100_000, maxLineBytes: 10 })

    expect(sent).toHaveLength(1)
  })

  it('a_small_unit_still_uses_one_message', async () => {
    const store = new MemoryStore()
    await storeUnit(store, [matchFrame(1001, false)])
    const { sent, sentParts, transport } = partsRecorder(() => ({ ok: true }))

    await sendClosedUnits(store, transport, settings, noDrop, { ...small, maxContentBytes: 100_000 })

    expect(sent).toHaveLength(1)
    expect(sentParts).toEqual([])
  })
})
