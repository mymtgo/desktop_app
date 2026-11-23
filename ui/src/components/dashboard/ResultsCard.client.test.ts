import { describe, expect, it, vi } from 'vitest'
import type { ResultsView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/components/decks/MatchHistoryChart.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: ['data', 'timeframe', 'title', 'storageKey', 'height'],
      setup: (props) => () => h('match-history-chart', { ...props }),
    }),
  }
})

const { default: ResultsCard } = await import('./ResultsCard.vue')

const RESULTS: ResultsView = {
  matches: 3,
  record: { wins: 2, losses: 1, draws: 0, total: 3, winRate: 67, label: '2 - 1' },
  games: { won: 5, lost: 3, winRate: 63 },
  streak: { current: '1W', bestWin: 2, bestLoss: 1 },
  matchWinRateDelta: -3,
  gameWinRateDelta: 4,
  playDraw: { otpWinRate: 70, otdWinRate: 50 },
  daily: [],
  recentSessions: [
    { day: '2026-11-23', daysAgo: 0, record: { wins: 2, losses: 1, draws: 0, total: 3, winRate: 67, label: '2 - 1' } },
    { day: '2026-11-22', daysAgo: 1, record: { wins: 0, losses: 2, draws: 0, total: 2, winRate: 0, label: '0 - 2' } },
  ],
}

describe('the Results card', () => {
  it('shows the numbers, the chart as the dashboard asks, and the recent sessions', () => {
    const mounted = mountFake(ResultsCard, { results: RESULTS, timeframe: 'week', emptyText: 'No matches in this timeframe' })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('67%')
    expect(text).toContain('-3%')
    expect(text).toContain('+4%')
    expect(text).toContain('Recent sessions')
    expect(text).not.toContain('Last 20')
    const chart = findAll(mounted.root, (node) => node.tag === 'match-history-chart')[0]
    expect(chart.props).toMatchObject({ title: '', storageKey: 'dashboard:results-chart-mode', height: 260 })
    const sessions = findAll(mounted.root, (node) => hasClass(node, 'recent-session'))
    expect(sessions.map((node) => textOf(node))).toEqual([
      expect.stringContaining('Today'),
      expect.stringContaining('Yesterday'),
    ])
    expect(textOf(sessions[0])).toContain('2 - 1')
    expect(textOf(sessions[0])).toContain('67%')
    expect(textOf(sessions[1])).toContain('0 - 2')
  })

  it('shows no change for all time and says when the window is empty', () => {
    const allTime = mountFake(ResultsCard, { results: RESULTS, timeframe: 'alltime', emptyText: 'x' })
    expect(textOf(allTime.root)).not.toContain('-3%')
    const empty = mountFake(ResultsCard, { results: { ...RESULTS, matches: 0 }, timeframe: 'week', emptyText: 'No Modern matches in this timeframe' })
    expect(textOf(empty.root)).toContain('No Modern matches in this timeframe')
    expect(findAll(empty.root, (node) => node.tag === 'match-history-chart')).toHaveLength(0)
  })
})
