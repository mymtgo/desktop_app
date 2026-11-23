import { describe, expect, it } from 'vitest'
import type { BreakdownView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: BreakdownCard } = await import('./BreakdownCard.vue')

function rec(wins: number, total: number) {
  return { wins, losses: total - wins, draws: 0, total, winRate: Math.round((100 * wins) / total), label: '' }
}

const BREAKDOWN: BreakdownView = {
  kind: 'formats',
  total: 577,
  rows: [
    { key: 'modern', label: 'Modern', matches: 352, share: 61, record: rec(151, 352) },
    { key: 'pioneer', label: 'Pioneer', matches: 225, share: 39, record: rec(135, 225) },
  ],
}

describe('the Formats card', () => {
  it('describes itself and labels the total, the matches and the win rate', () => {
    const mounted = mountFake(BreakdownCard, { breakdown: BREAKDOWN })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Share of your matches by format, and your match win rate in each.')
    expect(text).toContain('577')
    const rows = findAll(mounted.root, (node) => hasClass(node, 'breakdown-row')).map((node) =>
      textOf(node).replace(/\s+/g, ''),
    )
    expect(rows).toEqual(['Modern352matches43%winrate', 'Pioneer225matches60%winrate'])
  })
})
