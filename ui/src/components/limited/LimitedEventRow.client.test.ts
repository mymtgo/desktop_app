import { describe, expect, it, vi } from 'vitest'
import type { LimitedIndexRowView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ colourEnabled: [] as boolean[], unlinked: [] as unknown[][] }))
const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: (_props, { attrs }) => () => h(tag, { ...attrs }) }) }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  return {
    useLimitedRunColours: (_key: unknown, expanded: { value: boolean }) => {
      state.colourEnabled.push(expanded.value)
      return { data: ref(undefined) }
    },
  }
})
vi.mock('@/lib/limited', () => ({
  limitedMatchPath: () => '/x',
  limitedPath: () => '/x',
  rowInfo: () => ({ count: '2 packs', versions: null, avgPick: null, opponents: 'vs Qburn', note: null }),
  rowLinks: () => ({ title: null, deck: '/decks/1', draft: null }),
  withOpponentColours: (matches: unknown) => matches,
}))
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/leagues/LeagueResultBadge.vue', () => stub('result-badge'))
vi.mock('@/components/leagues/ResultDots.vue', () => stub('result-dots'))
vi.mock('@/components/leagues/RunMatchesTable.vue', () => stub('run-matches-table'))
vi.mock('@/components/leagues/LeagueRunActions.vue', () => stub('run-actions'))
vi.mock('@/bindings', () => ({
  commands: {
    unlinkLeagueMatch: (...args: unknown[]) => {
      state.unlinked.push(args)
      return { status: 'ok', data: null }
    },
  },
}))
vi.mock('@/composables/useLeagueEdit', async () => {
  const { ref } = await import('vue')
  return {
    useLeagueEdit: () => ({
      saving: ref(false),
      run: async (_title: string, action: () => unknown) => {
        await action()
        return true
      },
    }),
  }
})
vi.mock('@/components/leagues/RunSummaryStats.vue', () => stub('run-summary-stats'))

const { default: LimitedEventRow } = await import('./LimitedEventRow.vue')

const ROW = {
  runKey: 'q-draft:1',
  title: 'Q Set',
  setCode: 'QST',
  kind: 'draft',
  state: 'In progress',
  stateVariant: 'default',
  results: [],
  startedAtMs: null,
  linked: true,
  userState: null,
  autoState: 'active',
  notes: 'keep',
  matches: [{ token: 'q-1' }],
  gameWins: 0,
  gameLosses: 0,
  onPlayRecord: null,
  onDrawRecord: null,
  classification: 'LIVE',
  liveRound: 2,
  avgMatchSeconds: null,
  timeOfDay: null,
} as unknown as LimitedIndexRowView

const tags = (root: Parameters<typeof findAll>[0]) => findAll(root, () => true).map((node) => node.tag)

describe('the Limited event row', () => {
  it('opens a live run with its matches and a chevron by default', () => {
    const mounted = mountFake(LimitedEventRow, { row: ROW })
    expect(mounted.warnings).toEqual([])
    expect(tags(mounted.root)).toContain('run-matches-table')
    expect(findAll(mounted.root, (node) => node.props['aria-label'] === 'Hide matches')).toHaveLength(1)
  })

  it('uses the shared league header, with the event extras in its slots', () => {
    const mounted = mountFake(LimitedEventRow, { row: ROW })
    expect(tags(mounted.root)).toContain('result-badge')
    expect(tags(mounted.root)).toContain('result-dots')
    const text = textOf(mounted.root)
    for (const part of ['Q Set', 'Draft', '2 packs', 'vs Qburn', 'Deck']) expect(text).toContain(part)
    // In progress: no state badge.
    expect(text).not.toContain('In progress')
  })

  it('counts the matches the run record holds beyond the tracked ones', () => {
    expect(textOf(mountFake(LimitedEventRow, { row: ROW }).root)).not.toContain('played elsewhere')
    const two = mountFake(LimitedEventRow, { row: { ...ROW, untrackedMatches: 2 } })
    expect(two.warnings).toEqual([])
    expect(textOf(two.root)).toContain('2 matches played elsewhere')
    const one = mountFake(LimitedEventRow, { row: { ...ROW, untrackedMatches: 1 } })
    expect(textOf(one.root)).toContain('1 match played elsewhere')
  })

  it('has no actions for an unlinked draft', () => {
    const mounted = mountFake(LimitedEventRow, { row: { ...ROW, linked: false, matches: [] } })
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).not.toContain('Deck')
  })

  it('has no league menu for an unlinked draft', () => {
    const mounted = mountFake(LimitedEventRow, { row: { ...ROW, runKey: null, linked: false, matches: [] } })
    expect(tags(mounted.root)).not.toContain('run-actions')
  })

  it('shows the league menu for a linked run, fed from the row', () => {
    const mounted = mountFake(LimitedEventRow, { row: { ...ROW, userState: 'dropped' } })
    const [menu] = findAll(mounted.root, (node) => node.tag === 'run-actions')
    expect(menu.props).toMatchObject({
      'run-key': 'q-draft:1',
      'match-count': 1,
      manual: false,
      limited: '',
      shareable: false,
      'user-state': 'dropped',
      'auto-state': 'active',
      notes: 'keep',
    })
  })

  it('says "Set by you" only when the player set a status', () => {
    const set = mountFake(LimitedEventRow, { row: { ...ROW, userState: 'dropped' } })
    expect(findAll(set.root, (node) => node.props['data-testid'] === 'set-by-you')).toHaveLength(1)
    const auto = mountFake(LimitedEventRow, { row: ROW })
    expect(findAll(auto.root, (node) => node.props['data-testid'] === 'set-by-you')).toEqual([])
  })

  it('removes a match through unlinkLeagueMatch with the run key and token', async () => {
    const mounted = mountFake(LimitedEventRow, { row: ROW })
    const [table] = findAll(mounted.root, (node) => node.tag === 'run-matches-table')
    expect(table.props['can-unlink']).toBe(true)
    await (table.props.onUnlink as (token: string) => Promise<void>)('q-1')
    expect(state.unlinked).toEqual([['q-draft:1', 'q-1']])
  })

  it('offers only the status and hides match removal when the run is not editable', () => {
    const mounted = mountFake(LimitedEventRow, { row: ROW, editable: false })
    const [actions] = findAll(mounted.root, (node) => node.tag === 'run-actions')
    expect(actions.props['status-only']).toBe(true)
    const [arena] = findAll(
      mountFake(LimitedEventRow, { row: ROW, editable: false, platform: 'arena' }).root,
      (node) => node.tag === 'run-actions',
    )
    expect(arena.props.platform).toBe('arena')
    const [table] = findAll(mounted.root, (node) => node.tag === 'run-matches-table')
    expect(table.props['can-unlink']).toBe(false)
    expect(textOf(mounted.root)).toContain('Deck')
  })
})
