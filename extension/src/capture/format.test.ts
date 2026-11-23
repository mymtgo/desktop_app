import { describe, expect, it } from 'vitest'
import { frameLine, headerLine, type UnitHeader } from './format'

describe('frameLine', () => {
  it('emits body for JSON text, stripped', () => {
    const line = frameLine({ at: 5, src: 'game_ws', dir: 'in', url: '/ws', text: '{"a":1,"ticket":"x"}' })
    expect(line.includes('\n')).toBe(false)
    const o = JSON.parse(line)
    expect(o).toEqual({ at: 5, src: 'game_ws', dir: 'in', url: '/ws', body: { a: 1 } })
    expect('text' in o).toBe(false)
  })
  it('keeps text when not JSON', () => {
    const o = JSON.parse(frameLine({ at: 5, src: 'rest', dir: 'out', url: '/x', text: 'ping' }))
    expect(o.text).toBe('ping')
    expect('body' in o).toBe(false)
  })
})

describe('frameLine time', () => {
  it('truncates fractional at', () => {
    const o = JSON.parse(frameLine({ at: 1700000000000.7, src: 'rest', dir: 'in', url: '/x', text: '1' }))
    expect(o.at).toBe(1700000000000)
  })
})

describe('headerLine', () => {
  it('uses exactly the Rust CaptureHeader keys', () => {
    const h: UnitHeader = {
      mymtgo_endstep_capture: 1,
      unit_id: 'u',
      kind: 'match',
      started_at: 1,
      counter: 2,
      extension: '0.1.0',
      browser: 'chrome',
      developer: false,
      classification: 'unknown',
    }
    const o = JSON.parse(headerLine(h))
    expect(o.mymtgo_endstep_capture).toBe(1)
    expect(Object.keys(o).sort()).toEqual(
      ['mymtgo_endstep_capture', 'unit_id', 'kind', 'started_at', 'counter', 'extension', 'browser', 'developer', 'classification'].sort(),
    )
  })
})
