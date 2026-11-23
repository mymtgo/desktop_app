import { describe, expect, test } from 'vitest'
import ReplayBattlefield from './ReplayBattlefield.vue'
import { findAll, hasClass, mountFake } from './replayTestHarness'

// WebKitGTK 2.40 (the Linux floor, replays spec section 6.1) has no `safe`
// keyword in flex alignment, so the package's safe centring either drops (rows
// left-aligned) or clips an overflowing row's first cards. Each battlefield
// row centres with auto inline margins on its first and last item instead,
// which become 0 when the row overflows: safe centring on every engine.
// Built from parts so Tailwind, which scans this file too, emits no rule for it.
const SAFE_CENTRE = ['justify', 'center', 'safe'].join('-')
const sources = import.meta.glob<string>('./**/*.vue', { eager: true, query: '?raw', import: 'default' })

function rows(scroll: boolean, singleRow = false) {
  const mounted = mountFake(ReplayBattlefield, { battlefield: [], opponent: false, pairs: new Map(), scroll, singleRow })
  const found = findAll(mounted.root, (node) => 'data-replay-row' in node.props)
  expect(mounted.warnings).toEqual([])
  return found
}

describe('replay battlefield', () => {
  test('rows centre safely without the safe keyword', () => {
    for (const [scroll, singleRow] of [
      [false, false],
      [true, false],
      [true, true],
    ]) {
      const found = rows(scroll, singleRow)
      expect(found.length).toBeGreaterThan(0)
      for (const row of found) {
        expect(hasClass(row, SAFE_CENTRE)).toBe(false)
        expect(hasClass(row, 'justify-start')).toBe(true)
        expect(hasClass(row, '[&>:first-child]:ms-auto')).toBe(true)
        expect(hasClass(row, '[&>:last-child]:me-auto')).toBe(true)
      }
    }
  })

  test('no viewer file uses safe alignment', () => {
    expect(Object.keys(sources).length).toBeGreaterThan(30)
    for (const [file, text] of Object.entries(sources)) {
      expect(/-(?:center|end)-safe\b/.test(text), file).toBe(false)
    }
  })
})
