import { afterEach, describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: MatchHistoryChart } = await import('./MatchHistoryChart.vue')

/** A remembered chart mode, and the keys it was read and written under. */
function rememberedMode(mode: string) {
  const read: string[] = []
  vi.stubGlobal('localStorage', {
    getItem: (key: string) => {
      read.push(key)
      return mode
    },
    setItem: () => {},
  })
  return read
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the match history chart', () => {
  it('keeps the deck page as it was by default', () => {
    const read = rememberedMode('winrate')
    const mounted = mountFake(MatchHistoryChart, { data: [], timeframe: 'alltime' })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Performance History')
    expect(text).toContain('Sessions')
    expect(text).toContain('Trend')
    expect(text).not.toContain('cumulative')
    expect(read).toEqual(['deck:performance-chart-mode'])
    const sized = findAll(mounted.root, (node) => (node.props.style as { height?: string } | undefined)?.height === '400px')
    expect(sized).toHaveLength(1)
  })

  it('takes the dashboard title, storage key and height', () => {
    const read = rememberedMode('winrate')
    const mounted = mountFake(MatchHistoryChart, {
      data: [],
      timeframe: 'week',
      title: '',
      storageKey: 'dashboard:results-chart-mode',
      height: 260,
    })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).not.toContain('Performance History')
    expect(findAll(mounted.root, (node) => node.tag === 'h3')).toHaveLength(0)
    expect(read).toEqual(['dashboard:results-chart-mode'])
    const sized = findAll(mounted.root, (node) => (node.props.style as { height?: string } | undefined)?.height === '260px')
    expect(sized).toHaveLength(1)
  })
})
