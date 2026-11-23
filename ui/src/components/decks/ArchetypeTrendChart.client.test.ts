import { afterEach, describe, expect, it, vi } from 'vitest'
import type { DeckTrendView, PeerDayView } from '@/bindings'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

// The fake renderer has no layout, so the chart would never get a width and never draw its svg.
vi.mock('@vueuse/core', async () => {
  const { ref } = await import('vue')
  return { useElementSize: () => ({ width: ref(600), height: ref(400) }) }
})

const { default: ArchetypeTrendChart } = await import('./ArchetypeTrendChart.vue')

function stubStorage(mode: string | null) {
  const written: [string, string][] = []
  vi.stubGlobal('localStorage', {
    getItem: () => mode,
    setItem: (key: string, value: string) => written.push([key, value]),
  })
  return written
}

afterEach(() => {
  vi.unstubAllGlobals()
})

function deck(deckId: number, name: string, wins: number, losses: number, slot = deckId - 1): DeckTrendView {
  return { deckId, slot, name, wins, losses, draws: 0, days: [{ date: '2026-03-01', wins, losses, draws: 0, trend: 60 }] }
}

const decks = [deck(1, 'Burn A', 3, 1), deck(2, 'Burn B', 1, 1), deck(3, 'Burn C', 0, 2)]
const overall: PeerDayView[] = [
  { date: '2026-03-01', wins: 4, losses: 4, draws: 0, trend: 50 },
  { date: '2026-03-03', wins: 2, losses: 1, draws: 0, trend: 55 },
]

function groupsOf(root: FakeNode) {
  return findAll(root, (node) => node.tag === 'g' && node.props.class === 'chart-line')
}

function visiblePaths(root: FakeNode) {
  return findAll(root, (node) => node.tag === 'path' && node.props.stroke !== 'transparent')
}

function opacityOf(group: FakeNode): number {
  return (group.props.style as { opacity: number }).opacity
}

function legendEntries(root: FakeNode) {
  return findAll(root, (node) => node.tag === 'li')
}

function button(root: FakeNode, label: string): FakeNode {
  const found = findAll(root, (node) => node.tag === 'button' && textOf(node).trim() === label)
  expect(found).toHaveLength(1)
  return found[0]
}

describe('the archetype trend chart', () => {
  it('shows sessions and a trend by default, without a deck legend', () => {
    stubStorage(null)
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Sessions')
    expect(text).toContain('Trend')
    expect(legendEntries(mounted.root)).toHaveLength(0)
    const paths = visiblePaths(mounted.root)
    expect(paths.map((path) => path.props.class)).toEqual([
      'stroke-chart-1 pointer-events-none',
      'stroke-chart-indigo pointer-events-none',
    ])
  })

  it('draws a session dot for each day played and none for the empty day between', () => {
    stubStorage(null)
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    const dots = findAll(mounted.root, (node) => node.tag === 'circle' && String(node.props.class).includes('fill-chart-1'))
    expect(dots).toHaveLength(2)
    expect(Number(dots[0].props.r)).toBeGreaterThan(Number(dots[1].props.r))
  })

  it('shows a day with its own win rate, its record and the trend on hover', async () => {
    stubStorage(null)
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    const plot = findAll(mounted.root, (node) => node.tag === 'rect' && node.props.fill === 'transparent')[0]
    ;(plot.props.onMousemove as (event: unknown) => void)({ currentTarget: plot, clientX: 560 })
    await mounted.settle()
    const text = textOf(mounted.root)
    expect(text).toContain('67% win rate')
    expect(text).toContain('2W - 1L')
    expect(text).toContain('55%')
  })

  it('switches to a line per deck, with a legend entry for each, and remembers the choice', async () => {
    const written = stubStorage(null)
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    ;(button(mounted.root, 'Per deck').props.onClick as () => void)()
    await mounted.settle()

    const entries = legendEntries(mounted.root)
    expect(entries).toHaveLength(3)
    const text = textOf(mounted.root)
    expect(text).toContain('Burn A')
    expect(text).toContain('3W - 1L')
    expect(text).toContain('75%')
    expect(text).not.toContain('Sessions')
    expect(written).toEqual([['archetype:trends-mode', 'decks']])
    expect(visiblePaths(mounted.root)).toHaveLength(3)

    ;(button(mounted.root, 'Overall').props.onClick as () => void)()
    await mounted.settle()
    expect(legendEntries(mounted.root)).toHaveLength(0)
  })

  it('opens the deck of a legend entry', async () => {
    stubStorage('decks')
    const opened: number[] = []
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks, 'onOpen-deck': (id: number) => opened.push(id) })
    await mounted.settle()
    const entry = findAll(legendEntries(mounted.root)[1], (node) => node.tag === 'button')[0]
    ;(entry.props.onClick as () => void)()
    expect(opened).toEqual([2])
  })

  it('raises the hovered line, fades the rest, and clears when the decks change', async () => {
    stubStorage('decks')
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    await mounted.settle()
    expect(groupsOf(mounted.root).map(opacityOf)).toEqual([1, 1, 1])

    const hit = findAll(groupsOf(mounted.root)[1], (node) => node.tag === 'path' && node.props.stroke === 'transparent')[0]
    ;(hit.props.onMousemove as (event: unknown) => void)({ currentTarget: hit, clientX: 100 })
    await mounted.settle()
    const opacities = groupsOf(mounted.root).map(opacityOf)
    expect(opacities.filter((opacity) => opacity === 1)).toHaveLength(1)
    expect(opacities.filter((opacity) => opacity === 0.15)).toHaveLength(2)

    mounted.setProps({ decks: [deck(1, 'Burn A', 3, 1), deck(2, 'Burn B', 2, 1)] })
    await mounted.settle()
    expect(groupsOf(mounted.root).map(opacityOf)).toEqual([1, 1])
  })

  it('says the hovered deck\'s day record and trend in Per deck', async () => {
    stubStorage('decks')
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    await mounted.settle()
    const hit = findAll(groupsOf(mounted.root)[0], (node) => node.tag === 'path' && node.props.stroke === 'transparent')[0]
    ;(hit.props.onMousemove as (event: unknown) => void)({ currentTarget: hit, clientX: 0 })
    await mounted.settle()
    const text = textOf(mounted.root)
    expect(text).toContain('Burn A')
    expect(text).toContain('60% trend')
    expect(text).toContain('3W - 1L')
  })

  it('raises the line of a focused legend entry and keeps it through a pointer leave', async () => {
    stubStorage('decks')
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    await mounted.settle()
    const entry = findAll(legendEntries(mounted.root)[0], (node) => node.tag === 'button')[0]
    ;(entry.props.onFocus as () => void)()
    await mounted.settle()
    expect(groupsOf(mounted.root).map(opacityOf).filter((opacity) => opacity === 0.15)).toHaveLength(2)

    const list = findAll(mounted.root, (node) => node.tag === 'ul')[0]
    ;(list.props.onMouseleave as () => void)()
    await mounted.settle()
    expect(groupsOf(mounted.root).map(opacityOf).filter((opacity) => opacity === 0.15)).toHaveLength(2)

    ;(entry.props.onBlur as () => void)()
    await mounted.settle()
    expect(groupsOf(mounted.root).map(opacityOf)).toEqual([1, 1, 1])
  })

  it('keeps each deck stroke when the lowest-id deck drops out of the range', async () => {
    stubStorage('decks')
    const strokes = (root: FakeNode) => visiblePaths(root).map((path) => path.props.stroke)
    const mounted = mountFake(ArchetypeTrendChart, { overall, decks })
    await mounted.settle()
    const before = strokes(mounted.root)

    mounted.setProps({ decks: [deck(2, 'Burn B', 1, 1), deck(3, 'Burn C', 0, 2)] })
    await mounted.settle()
    expect(strokes(mounted.root)).toEqual(before.slice(1))
  })

  it('says so when no deck has a match in the timeframe', () => {
    stubStorage('decks')
    const mounted = mountFake(ArchetypeTrendChart, { overall: [], decks: [] })
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toContain('No matches in this timeframe')
    expect(findAll(mounted.root, (node) => node.tag === 'button')).toHaveLength(0)
  })
})
