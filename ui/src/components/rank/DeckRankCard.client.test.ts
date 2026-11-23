import { describe, expect, it } from 'vitest'
import type { DeckRankView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: DeckRankCard } = await import('./DeckRankCard.vue')

const RANK: DeckRankView = {
  line: {
    ladder: 'constructed',
    season: 1,
    seasonStartMs: null,
    seasonEndMs: null,
    current: 'Silver 2',
    points: [
      { atMs: 1_000, steps: 30, label: 'Silver 3', mythic: false },
      { atMs: 2_000, steps: 36, label: 'Silver 2', mythic: false },
    ],
    gaps: [{ from: 0, to: 1, kind: 'attributed', changeSteps: 6, ofDeck: true, bestOf: null }],
    matches: [{ token: 'a', atMs: 1_500, outcome: 'win', ofDeck: true, bestOf: null, manual: false, gap: 0 }],
  },
  climb: { direction: 'climbed', text: '1 tier', mythicText: null, attributed: 1 },
}

describe('the deck rank card', () => {
  it('shows the climb sentence', () => {
    const mounted = mountFake(DeckRankCard, { rank: RANK })
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toContain('Climbed 1 tier with this deck this season')
    // The chart highlights the deck's gaps.
    expect(findAll(mounted.root, (node) => node.tag === 'svg')).toHaveLength(1)
  })

  it('with no rank reading still shows the card', () => {
    const mounted = mountFake(DeckRankCard, { rank: null })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Constructed rank')
    expect(text).toContain('No rank reading this season yet.')
    expect(findAll(mounted.root, (node) => node.tag === 'svg')).toEqual([])
  })

  it('a failed read says so', () => {
    const mounted = mountFake(DeckRankCard, { rank: null, error: 'disk on fire' })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Could not load the rank: disk on fire')
    expect(text).not.toContain('No rank reading')
  })
})
