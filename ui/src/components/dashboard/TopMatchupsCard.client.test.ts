import { describe, expect, it, vi } from 'vitest'
import type { TopMatchupRowView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: TopMatchupsCard } = await import('./TopMatchupsCard.vue')

const ROW = {
  archetypeId: 1,
  name: 'Qburn',
  share: 4,
  record: { wins: 15, losses: 9, draws: 0, total: 24, winRate: 63, label: '' },
} as unknown as TopMatchupRowView

describe('the Top matchups card', () => {
  it('shows the share as text, the record, and the win rate as the bar', () => {
    const mounted = mountFake(TopMatchupsCard, { matchups: [ROW] })
    expect(mounted.warnings).toEqual([])
    const share = findAll(mounted.root, (node) => hasClass(node, 'share-text')).map((node) => textOf(node).trim())
    expect(share).toEqual(['24 matches · 4% of matches'])
    const text = textOf(mounted.root)
    expect(text).toContain('15W')
    expect(text).toContain('63%')
    expect(findAll(mounted.root, (node) => hasClass(node, 'meter-fill'))).toHaveLength(1)
  })
})
