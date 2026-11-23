import { describe, expect, it } from 'vitest'
import type { RankLineView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: RankChart } = await import('./RankChart.vue')

const LINE: RankLineView = {
  ladder: 'constructed',
  season: 1,
  seasonStartMs: 0,
  seasonEndMs: 10_000,
  current: 'Gold 4',
  points: [
    { atMs: 1_000, steps: 30, label: 'Silver 3', mythic: false },
    { atMs: 2_000, steps: 36, label: 'Silver 2', mythic: false },
    { atMs: 4_000, steps: 50, label: 'Gold 4', mythic: false },
  ],
  gaps: [
    { from: 0, to: 1, kind: 'attributed', changeSteps: 6, ofDeck: true, bestOf: null },
    { from: 1, to: 2, kind: 'played_elsewhere', changeSteps: 14, ofDeck: false, bestOf: null },
  ],
  matches: [
    { token: 'deck', atMs: 1_500, outcome: 'win', ofDeck: true, bestOf: null, manual: false, gap: 0 },
    { token: 'other', atMs: 4_500, outcome: 'loss', ofDeck: false, bestOf: null, manual: false, gap: null },
    { token: 'manual', atMs: 4_600, outcome: 'win', ofDeck: false, bestOf: null, manual: true, gap: null },
  ],
}

describe('the rank chart', () => {
  it('played elsewhere gaps are dashed and titled', () => {
    const mounted = mountFake(RankChart, { line: LINE, highlightDeck: true })
    expect(mounted.warnings).toEqual([])
    const dashed = findAll(mounted.root, (node) => node.tag === 'path' && node.props['stroke-dasharray'] !== undefined)
    expect(dashed).toHaveLength(1)
    const titles = findAll(mounted.root, (node) => node.tag === 'title').map((node) => textOf(node))
    expect(titles).toContain('Played elsewhere')
    const svg = findAll(mounted.root, (node) => node.tag === 'svg')[0]
    expect(String(svg.props['aria-label'])).toContain('Gold 4')
  })

  it('the chart draws no match markers or reading dots', () => {
    const mounted = mountFake(RankChart, { line: LINE, highlightDeck: true })
    expect(LINE.matches.length).toBeGreaterThan(0)
    expect(findAll(mounted.root, (node) => node.tag === 'circle' || node.tag === 'rect')).toEqual([])
    expect(findAll(mounted.root, (node) => node.props['data-token'] !== undefined)).toEqual([])
  })

  it('tier changes draw their rank icon', () => {
    const mounted = mountFake(RankChart, { line: LINE, highlightDeck: true })
    const images = findAll(mounted.root, (node) => node.tag === 'image')
    expect(images).toHaveLength(3)
    expect(String(images[0].props.href)).toContain('constructed_silver_t3')
    const titles = findAll(images[2], (node) => node.tag === 'title').map((node) => textOf(node))
    expect(titles).toEqual(['Gold 4'])
  })

  it('the limited line is drawn between its readings', () => {
    const limited: RankLineView = {
      ladder: 'limited',
      season: 1,
      seasonStartMs: 0,
      seasonEndMs: 10_000,
      current: 'Silver 1',
      points: [
        { atMs: 1_000, steps: 30, label: 'Silver 3', mythic: false },
        { atMs: 2_000, steps: 36, label: 'Silver 2', mythic: false },
        { atMs: 3_000, steps: 42, label: 'Silver 1', mythic: false },
      ],
      gaps: [
        { from: 0, to: 1, kind: 'quiet', changeSteps: 6, ofDeck: false, bestOf: null },
        { from: 1, to: 2, kind: 'quiet', changeSteps: 6, ofDeck: false, bestOf: null },
      ],
      matches: [],
    }
    const mounted = mountFake(RankChart, { line: limited })
    expect(mounted.warnings).toEqual([])
    const drawn = findAll(mounted.root, (node) => node.tag === 'path' && node.props.stroke !== 'transparent')
    expect(drawn).toHaveLength(limited.points.length - 1)
    expect(findAll(mounted.root, (node) => node.tag === 'path' && node.props['stroke-dasharray'] !== undefined)).toEqual(
      [],
    )
  })

  it('the home chart colours Bo3 and Bo1 apart and the legend lists each kind present', () => {
    const line: RankLineView = {
      ...LINE,
      gaps: [
        { from: 0, to: 1, kind: 'attributed', changeSteps: 6, ofDeck: false, bestOf: 'bo3' },
        { from: 1, to: 2, kind: 'attributed', changeSteps: 14, ofDeck: false, bestOf: 'bo1' },
      ],
    }
    const mounted = mountFake(RankChart, { line })
    expect(mounted.warnings).toEqual([])
    expect(findAll(mounted.root, (node) => hasClass(node, 'rank-bo3')).length).toBeGreaterThan(0)
    expect(findAll(mounted.root, (node) => hasClass(node, 'rank-bo1')).length).toBeGreaterThan(0)
    const drawn = findAll(mounted.root, (node) => node.tag === 'path' && node.props.stroke !== 'transparent')
    expect(String(drawn[0].props.class)).not.toEqual(String(drawn[1].props.class))
    const text = textOf(mounted.root)
    expect(text).toContain('Bo3')
    expect(text).toContain('Bo1')
    expect(text).not.toContain('Played elsewhere')
  })

  it('a kindless attributed stretch keeps the neutral colour and no kind legend', () => {
    const mounted = mountFake(RankChart, { line: LINE })
    expect(findAll(mounted.root, (node) => hasClass(node, 'rank-bo3') || hasClass(node, 'rank-bo1'))).toEqual([])
    const text = textOf(mounted.root)
    expect(text).not.toContain('Bo3')
    expect(text).not.toContain('Bo1')
    expect(text).toContain('Played elsewhere')
  })

  it('the deck chart draws a Bo1 stretch thinner than Bo3 and keeps the accent', () => {
    const line: RankLineView = {
      ...LINE,
      gaps: [
        { from: 0, to: 1, kind: 'attributed', changeSteps: 6, ofDeck: true, bestOf: 'bo3' },
        { from: 1, to: 2, kind: 'attributed', changeSteps: 14, ofDeck: true, bestOf: 'bo1' },
      ],
    }
    const mounted = mountFake(RankChart, { line, highlightDeck: true })
    const drawn = findAll(mounted.root, (node) => node.tag === 'path' && node.props.stroke !== 'transparent')
    expect(drawn).toHaveLength(2)
    expect(Number(drawn[1].props['stroke-width'])).toBeLessThan(Number(drawn[0].props['stroke-width']))
    expect(hasClass(drawn[0], 'stroke-primary')).toBe(true)
    expect(hasClass(drawn[1], 'stroke-primary')).toBe(true)
  })
})
