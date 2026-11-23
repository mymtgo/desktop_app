import { defineComponent, h } from 'vue'
import { describe, expect, it, vi } from 'vitest'
import type { LeagueRunHeaderView } from '@/lib/leagueRunHeader'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      inheritAttrs: false,
      props: { colors: Array, classification: String, wins: Number, losses: Number, results: Array },
      setup: (props) => () => h(tag, { ...props }),
    }),
  }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/leagues/LeagueResultBadge.vue', () => stub('result-badge'))
vi.mock('@/components/leagues/ResultDots.vue', () => stub('result-dots'))

const { default: LeagueRunHeader } = await import('./LeagueRunHeader.vue')

const HEADER: LeagueRunHeaderView = {
  classification: 'CASH',
  wins: 4,
  losses: 1,
  liveRound: null,
  results: ['W', 'W', 'L', 'W', 'W'],
  name: 'Qburn',
  link: '/decks/7',
  colors: ['R'],
  format: 'Modern',
  manual: false,
  versionLabel: 'v2',
  state: null,
  startedAtMs: 0,
  avgMatchSeconds: 1500,
  timeOfDay: 'evening',
  tixDelta: 12.7,
}

/** The header with its slots filled with marker elements. */
function withSlots(header: LeagueRunHeaderView) {
  return defineComponent({
    setup: () => () =>
      h(
        LeagueRunHeader,
        { header, now: 1000 },
        {
          'title-extra': () => h('x-title-extra'),
          'info-extra': () => h('x-info-extra'),
          actions: () => h('x-actions'),
          default: () => h('x-below'),
        },
      ),
  })
}

const tags = (root: Parameters<typeof findAll>[0]) => findAll(root, () => true).map((node) => node.tag)

describe('the league run header', () => {
  it('shows the score, identity, version, dots and the info line', () => {
    const mounted = mountFake(LeagueRunHeader, { header: HEADER, now: 1000 })
    expect(mounted.warnings).toEqual([])
    const badge = findAll(mounted.root, (node) => node.tag === 'result-badge')
    expect(badge[0].props).toMatchObject({ classification: 'CASH', wins: 4, losses: 1 })
    expect(findAll(mounted.root, (node) => node.tag === 'mana-colors')[0].props.colors).toEqual(['R'])
    expect(findAll(mounted.root, (node) => node.tag === 'a').map((link) => link.props.href)).toEqual(['/decks/7'])
    expect(tags(mounted.root)).toContain('result-dots')
    const text = textOf(mounted.root)
    for (const part of ['Qburn', 'Modern', 'v2', 'avg', '25m', 'Evening', '+13 tix']) expect(text).toContain(part)
    expect(text).not.toContain('Manual')
    expect(text).not.toContain('Dropped')
  })

  it('renders every slot in its place', () => {
    const mounted = mountFake(withSlots(HEADER))
    expect(mounted.warnings).toEqual([])
    const order = tags(mounted.root).filter((tag) => tag.startsWith('x-') || tag === 'result-dots')
    expect(order).toEqual(['result-dots', 'x-title-extra', 'x-info-extra', 'x-below', 'x-actions'])
  })

  it('shows the name as text without a link, and no identity without a name', () => {
    const plain = mountFake(LeagueRunHeader, { header: { ...HEADER, link: null }, now: 1000 })
    expect(findAll(plain.root, (node) => node.tag === 'a')).toEqual([])
    expect(textOf(plain.root)).toContain('Qburn')

    const hidden = mountFake(LeagueRunHeader, {
      header: { ...HEADER, name: null, link: null, colors: [], format: null },
      now: 1000,
    })
    expect(tags(hidden.root)).not.toContain('mana-colors')
    expect(textOf(hidden.root)).not.toContain('Modern')
  })

  it('badges the state it is given, with its hover text, and a manual league', () => {
    const mounted = mountFake(LeagueRunHeader, {
      header: { ...HEADER, manual: true, state: { label: 'Dropped', tone: 'destructive', title: 'Dropped 2 days ago' } },
      now: 1000,
    })
    const text = textOf(mounted.root)
    expect(text).toContain('Manual')
    expect(text).toContain('Dropped')
    expect(findAll(mounted.root, (node) => node.props.title === 'Dropped 2 days ago')).toHaveLength(1)

    const warning = mountFake(LeagueRunHeader, {
      header: { ...HEADER, state: { label: 'Unlinked', tone: 'warning', title: null } },
      now: 1000,
    })
    expect(textOf(warning.root)).toContain('Unlinked')
  })

  it('leaves out the date without a start, and the tix without a price', () => {
    const mounted = mountFake(LeagueRunHeader, {
      header: { ...HEADER, startedAtMs: null, tixDelta: null, avgMatchSeconds: null, timeOfDay: null },
      now: 1000,
    })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).not.toContain('tix')
    expect(text).not.toContain('avg')
    expect(text).not.toContain('ago')
  })
})
