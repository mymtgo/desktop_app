import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import type { Frame } from './format'
import { UnitTracker, type UnitAction } from './units'

const UNIT = new URL('../../../fixtures/endstep/ranked-modern-bo3/04-match-946164b1.jsonl', import.meta.url)

function frames(): Frame[] {
  const lines = readFileSync(UNIT, 'utf8').split('\n').slice(1).filter((line) => line !== '')
  return lines.map((line) => {
    const { at, src, dir, url, body, text } = JSON.parse(line)
    return { at, src, dir, url, text: text ?? JSON.stringify(body) }
  })
}

describe('the ranked Bo3 fixture', () => {
  it('closes its match unit at the lobby GAME_OVER, before the socket closes', () => {
    let n = 0
    const t = new UnitTracker(() => `u${++n}`)
    t.onEvent(1, { kind: 'socket_open', socketId: 's1', url: '/play/forge-b-k-4/ws' }, 0)
    const all = frames()
    let closedAt = -1
    const after: UnitAction[] = []
    all.forEach((frame, index) => {
      const socketId = frame.src === 'game_ws' ? 's1' : undefined
      const actions = t.onEvent(1, { kind: 'frame', socketId, frame }, frame.at)
      if (closedAt >= 0) after.push(...actions)
      if (actions.some((a) => a.kind === 'close' && a.unitId === 'u1')) closedAt = index
    })
    expect(JSON.parse(all[closedAt].text).type).toBe('GAME_OVER')
    expect(all.length - closedAt - 1).toBe(4)
    expect(after.filter((a) => a.kind === 'append' && a.unitId === 'u1')).toEqual([])
    expect(t.onEvent(1, { kind: 'socket_close', socketId: 's1' }, 0)).toEqual([])
  })
})
