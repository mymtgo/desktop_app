import { describe, expect, it, vi } from 'vitest'
import type { RunCardView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ pushed: [] as unknown[] }))
const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      inheritAttrs: false,
      props: {
        colors: Array,
        canDelete: Boolean,
        canAddMatches: Boolean,
        userState: String,
        autoState: String,
        disabled: Boolean,
        canUnlink: Boolean,
      },
      setup: (props) => () => h(tag, { ...props }),
    }),
  }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return {
    RouterLink: RouterLinkStub,
    useRouter: () => ({
      push: async (to: unknown) => {
        state.pushed.push(to)
      },
    }),
  }
})
vi.mock('@/bindings', () => ({ commands: {} }))
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/leagues/DeleteLeagueDialog.vue', () => stub('delete-dialog'))
vi.mock('@/components/leagues/DropLeagueDialog.vue', () => stub('drop-dialog'))
vi.mock('@/components/leagues/LeagueContextMenu.vue', () => stub('league-menu'))
vi.mock('@/components/leagues/LeagueNotes.vue', () => stub('league-notes'))
vi.mock('@/components/leagues/LeagueResultBadge.vue', () => stub('result-badge'))
vi.mock('@/components/leagues/LinkMatchesDialog.vue', () => stub('link-dialog'))
vi.mock('@/components/leagues/ResultDots.vue', () => stub('result-dots'))
vi.mock('@/components/leagues/RunMatchesTable.vue', () => stub('run-matches-table'))
vi.mock('@/components/leagues/RunSummaryStats.vue', () => stub('run-summary-stats'))
vi.mock('@/components/screenshots/LeagueScreenshot.vue', () => stub('league-screenshot'))
vi.mock('@/composables/useLeagueEdit', async () => {
  const { ref } = await import('vue')
  return { useLeagueEdit: () => ({ saving: ref(false), run: async () => {} }) }
})
vi.mock('@/composables/useLeagueManage', async () => {
  const { ref } = await import('vue')
  return { useLeagueManage: () => ({ saving: ref(false), setStatus: async () => true }) }
})
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))
vi.mock('@/composables/useOffscreenScreenshot', async () => {
  const { ref } = await import('vue')
  return {
    loadLeagueCover: async () => null,
    useOffscreenScreenshot: () => ({
      shot: ref(null),
      busy: ref(false),
      saveOffered: ref(false),
      saving: ref(false),
      target: ref(null),
      take: async () => {},
      save: () => {},
    }),
  }
})

const { default: LeagueCard } = await import('./LeagueCard.vue')

const CONSTRUCTED = {
  key: 'q-run:1',
  source: 'log',
  name: 'Qleague',
  format: 'Modern',
  formatCode: 'CMODERN',
  state: 'active',
  userState: null,
  autoState: 'active',
  manual: false,
  startedAtMs: 0,
  droppedAtMs: null,
  deck: { id: 7, name: 'Qburn', colors: ['R'], coverArt: null, localCoverArt: null },
  versionLabel: 'v2',
  results: [{ result: 'W' }, null, null, null, null],
  matches: [{ token: 'q-m1' }],
  notes: null,
  classification: 'LIVE',
  liveRound: 2,
  avgMatchSeconds: null,
  timeOfDay: null,
  gameWins: 2,
  gameLosses: 0,
  onPlayRecord: { wins: 1, losses: 0 },
  onDrawRecord: { wins: 1, losses: 0 },
  tixDelta: null,
  topOpponentArchetype: null,
  topMatchups: [],
  limited: false,
  colors: null,
} as unknown as RunCardView

const LIMITED = {
  ...CONSTRUCTED,
  key: 'q-draft:2',
  name: 'QNW Draft',
  format: 'Draft',
  formatCode: 'DQNWQNWQNW',
  deck: null,
  versionLabel: null,
  results: [{ result: 'W' }, null, null],
  limited: true,
  colors: ['U', 'R'],
} as unknown as RunCardView

const tags = (root: Parameters<typeof findAll>[0]) => findAll(root, () => true).map((node) => node.tag)
const chevrons = (root: Parameters<typeof findAll>[0]) =>
  findAll(root, (node) => typeof node.props['aria-expanded'] !== 'undefined')

describe('the Leagues page run card', () => {
  it('opens a live run with its chevron and body by default', () => {
    const mounted = mountFake(LeagueCard, { league: CONSTRUCTED, now: 1000 })
    expect(mounted.warnings).toEqual([])
    expect(chevrons(mounted.root)).toHaveLength(1)
    expect(tags(mounted.root)).toContain('run-summary-stats')
  })

  it('compact keeps the header, with no chevron and no body, even for a live run', () => {
    const mounted = mountFake(LeagueCard, { league: CONSTRUCTED, now: 1000, compact: true })
    expect(mounted.warnings).toEqual([])
    expect(chevrons(mounted.root)).toEqual([])
    expect(tags(mounted.root)).not.toContain('run-summary-stats')
    expect(tags(mounted.root)).not.toContain('run-matches-table')
    expect(tags(mounted.root)).toContain('result-badge')
    expect(tags(mounted.root)).toContain('result-dots')
    expect(tags(mounted.root)).toContain('league-menu')
    expect(textOf(mounted.root)).toContain('Qburn')
    expect(textOf(mounted.root)).toContain('Modern')
    expect(textOf(mounted.root)).toContain('v2')
  })

  it('compact opens the deck on a click on a constructed run', () => {
    state.pushed = []
    const mounted = mountFake(LeagueCard, { league: CONSTRUCTED, compact: true })
    const header = findAll(mounted.root, (node) => typeof node.props.onClick === 'function' && node.tag === 'div')[0]
    ;(header.props.onClick as () => void)()
    expect(state.pushed).toEqual(['/decks/7'])
  })

  it('shows a limited run by its set name, colours and format, linking to its event', () => {
    state.pushed = []
    const mounted = mountFake(LeagueCard, { league: LIMITED, now: 1000, compact: true })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('QNW Draft')
    expect(text).toContain('Draft')
    const colours = findAll(mounted.root, (node) => node.tag === 'mana-colors')
    expect(colours).toHaveLength(1)
    expect(colours[0].props.colors).toEqual(['U', 'R'])
    const links = findAll(mounted.root, (node) => node.tag === 'a')
    expect(links.map((link) => link.props.href)).toEqual(['/limited/q-draft%3A2/draft'])
    const header = findAll(mounted.root, (node) => typeof node.props.onClick === 'function' && node.tag === 'div')[0]
    ;(header.props.onClick as () => void)()
    expect(state.pushed).toEqual(['/limited/q-draft%3A2/draft'])
  })

  it('opens a sealed run on its deck page', () => {
    const sealed = { ...LIMITED, name: 'QOD Sealed', format: 'Sealed' } as RunCardView
    const mounted = mountFake(LeagueCard, { league: sealed, compact: true })
    const links = findAll(mounted.root, (node) => node.tag === 'a')
    expect(links.map((link) => link.props.href)).toEqual(['/limited/q-draft%3A2/deck'])
  })

  it('offers status, delete, Add matches and removing a match for a limited run, but no manual match', () => {
    const complete = { ...LIMITED, state: 'complete', autoState: 'complete' } as RunCardView
    const mounted = mountFake(LeagueCard, { league: complete, canCreateMatch: true, defaultExpanded: true })
    expect(mounted.warnings).toEqual([])
    const menu = findAll(mounted.root, (node) => node.tag === 'league-menu')
    expect(menu).toHaveLength(1)
    expect(menu[0].props.canDelete).toBe(true)
    expect(menu[0].props.canAddMatches).toBe(true)
    expect(findAll(mounted.root, (node) => node.tag === 'run-matches-table')[0].props.canUnlink).toBe(true)
    expect(textOf(mounted.root)).not.toContain('Add manual match')
  })

  it('keeps delete off an active limited run', () => {
    const mounted = mountFake(LeagueCard, { league: { ...LIMITED, state: 'active' } as RunCardView })
    expect(findAll(mounted.root, (node) => node.tag === 'league-menu')[0].props.canDelete).toBe(false)
  })

  it('keeps delete off an active run but lets it add matches and take a manual match', () => {
    const mounted = mountFake(LeagueCard, { league: CONSTRUCTED, canCreateMatch: true })
    const menu = findAll(mounted.root, (node) => node.tag === 'league-menu')[0]
    expect(menu.props.canDelete).toBe(false)
    expect(menu.props.canAddMatches).toBe(true)
    expect(menu.props.autoState).toBe('active')
    expect(textOf(mounted.root)).toContain('Add manual match')
  })

  it('offers delete once the run reads Complete, and removing a match from any constructed run', () => {
    const complete = { ...CONSTRUCTED, state: 'complete', autoState: 'complete', classification: 'CASH' } as RunCardView
    const mounted = mountFake(LeagueCard, { league: complete, defaultExpanded: true })
    expect(findAll(mounted.root, (node) => node.tag === 'league-menu')[0].props.canDelete).toBe(true)
    expect(findAll(mounted.root, (node) => node.tag === 'run-matches-table')[0].props.canUnlink).toBe(true)
  })

  it('tags a status the player set', () => {
    const set = { ...CONSTRUCTED, state: 'partial', userState: 'partial', classification: 'FINISH' } as RunCardView
    const mounted = mountFake(LeagueCard, { league: set })
    const [tag] = findAll(mounted.root, (node) => node.props['data-testid'] === 'set-by-you')
    expect(textOf(tag)).toBe('Set by you')
    expect(tag.props.title).toBe('Status set by you: Partial')
    const automatic = mountFake(LeagueCard, { league: CONSTRUCTED })
    expect(findAll(automatic.root, (node) => node.props['data-testid'] === 'set-by-you')).toEqual([])
  })
})
