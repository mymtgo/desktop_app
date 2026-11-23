import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { DeletedItemsView } from '@/bindings'
import { startedDayMonth } from '@/lib/leagues'
import { matchDateText } from '@/lib/matchList'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const OCT_11 = Date.UTC(2026, 9, 11, 12)
const OCT_12 = Date.UTC(2026, 9, 12, 18)

type Pending = { key: string; finish: () => void }

const state = vi.hoisted(() => ({
  view: { leagues: [], matches: [] } as DeletedItemsView,
  pending: [] as Pending[],
}))

/** A restore that waits until the test finishes it, so the busy row can be seen. */
function waitingRestore(key: string): Promise<boolean> {
  return new Promise((resolve) => state.pending.push({ key, finish: () => resolve(true) }))
}

vi.mock('@/lib/queries', async () => {
  const { computed, ref } = await import('vue')
  return { useDeletedItems: () => ({ data: computed(() => state.view), isPending: ref(false) }) }
})
vi.mock('@/composables/useLeagueManage', async () => {
  const { ref } = await import('vue')
  return { useLeagueManage: () => ({ saving: ref(false), restoreLeague: (key: string) => waitingRestore(key) }) }
})
vi.mock('@/composables/useMatchEdit', async () => {
  const { ref } = await import('vue')
  return { useMatchEdit: () => ({ saving: ref(false), restore: (token: string) => waitingRestore(token) }) }
})
// reka's tabs need a DOM: these stand-ins keep the page's v-model and show
// only the selected tab's content.
vi.mock('@/components/ui/tabs', async () => {
  const { computed, defineComponent, h, inject, provide } = await import('vue')
  const KEY = Symbol('tabs')
  type Ctx = { selected: { value: string }; select: (value: string) => void }
  const Tabs = defineComponent({
    props: { modelValue: { type: String, required: true } },
    emits: ['update:modelValue'],
    setup(props, { emit, slots }) {
      const selected = computed(() => props.modelValue)
      provide(KEY, { selected, select: (value: string) => emit('update:modelValue', value) })
      return () => h('tabs', slots.default?.())
    },
  })
  const TabsList = defineComponent({ setup: (_, { slots }) => () => h('tabs-list', slots.default?.()) })
  const TabsTrigger = defineComponent({
    props: { value: { type: String, required: true } },
    setup(props, { slots }) {
      const ctx = inject(KEY) as Ctx
      return () => h('tab-trigger', { 'data-tab': props.value, onClick: () => ctx.select(props.value) }, slots.default?.())
    },
  })
  const TabsContent = defineComponent({
    props: { value: { type: String, required: true } },
    setup(props, { slots }) {
      const ctx = inject(KEY) as Ctx
      return () => (ctx.selected.value === props.value ? h('tab-content', slots.default?.()) : null)
    },
  })
  return { Tabs, TabsList, TabsTrigger, TabsContent }
})

const { default: ArchivesPage } = await import('./ArchivesPage.vue')

beforeEach(() => {
  state.view = {
    leagues: [{ runKey: 'q:1', name: 'Qleague', startedAtMs: OCT_11, removedAtMs: OCT_12, wins: 4, losses: 1, deckName: 'Qdeck', format: 'Modern' }],
    matches: [
      { token: 'q-m1', startedAtMs: OCT_11, hiddenAtMs: OCT_12, opponent: 'Qopponent', result: 'loss', deckName: 'Qdeck', leagueName: null, leagueRemoved: false, deletedWithLeague: null },
      { token: 'q-m2', startedAtMs: OCT_11 + 1, hiddenAtMs: OCT_12 - 1, opponent: null, result: 'win', deckName: null, leagueName: null, leagueRemoved: false, deletedWithLeague: null },
    ],
  }
  state.pending = []
})

function triggers(root: FakeNode) {
  return findAll(root, (node) => node.tag === 'tab-trigger')
}

function restoreButtons(root: FakeNode, kind: string) {
  return findAll(root, (node) => node.props['data-restore'] === kind)
}

function click(node: FakeNode) {
  ;(node.props.onClick as () => void)()
}

describe('the Archives page', () => {
  it('shows the Matches and Leagues tabs with their counts, Matches first', () => {
    const mounted = mountFake(ArchivesPage)
    expect(mounted.warnings).toEqual([])
    expect(triggers(mounted.root).map((node) => [node.props['data-tab'], textOf(node).trim()])).toEqual([
      ['matches', 'Matches (2)'],
      ['leagues', 'Leagues (1)'],
    ])
  })

  it('lists the deleted matches in a table, newest deleted first', () => {
    const mounted = mountFake(ArchivesPage)
    const text = textOf(mounted.root)
    for (const heading of ['Date', 'Opponent', 'Result', 'Deleted']) expect(text).toContain(heading)
    expect(text).toContain(matchDateText(OCT_11))
    expect(text).toContain(matchDateText(OCT_12))
    expect(text).toContain('Qopponent')
    expect(text).toContain('Unknown opponent')
    expect(text).toContain('Loss')
    expect(text.indexOf('Qopponent')).toBeLessThan(text.indexOf('Unknown opponent'))
    expect(restoreButtons(mounted.root, 'match')).toHaveLength(2)
    expect(text).not.toContain('Qleague')
  })

  it('switches to the Leagues table', async () => {
    const mounted = mountFake(ArchivesPage)
    click(triggers(mounted.root)[1])
    await mounted.settle()
    const text = textOf(mounted.root)
    for (const heading of ['League', 'Started', 'Record', 'Deleted']) expect(text).toContain(heading)
    expect(text).toContain('Qleague')
    expect(text).toContain(startedDayMonth(OCT_11))
    expect(text).toContain('4-1')
    expect(text).toContain(matchDateText(OCT_12))
    expect(text).not.toContain('Qopponent')
    expect(restoreButtons(mounted.root, 'league')).toHaveLength(1)
    expect(text).toContain('Restore brings a league back with its matches, those deleted with it too.')
  })

  it('restores a match by its token, busy on that row only', async () => {
    const mounted = mountFake(ArchivesPage)
    click(restoreButtons(mounted.root, 'match')[1])
    await mounted.settle()
    expect(state.pending.map((each) => each.key)).toEqual(['q-m2'])
    expect(restoreButtons(mounted.root, 'match').map((node) => node.props.disabled)).toEqual([false, true])
    state.pending[0].finish()
    await mounted.settle()
    expect(restoreButtons(mounted.root, 'match').map((node) => node.props.disabled)).toEqual([false, false])
  })

  it('offers Restore league instead of Restore for a match deleted with its league', async () => {
    state.view.matches.push({
      token: 'q-m3',
      startedAtMs: OCT_11 + 2,
      hiddenAtMs: OCT_12 - 2,
      opponent: 'Qwith',
      result: 'win',
      deckName: 'Qdeck',
      leagueName: 'Qleague',
      leagueRemoved: true,
      deletedWithLeague: { runKey: 'q:1', name: 'Qleague' },
    })
    const mounted = mountFake(ArchivesPage)
    expect(mounted.warnings).toEqual([])
    expect(restoreButtons(mounted.root, 'match')).toHaveLength(2)
    const [withLeague] = restoreButtons(mounted.root, 'match-league')
    expect(textOf(withLeague).trim()).toBe('Restore league')
    expect(textOf(mounted.root)).toContain('Deleted with its league')
    click(withLeague)
    await mounted.settle()
    expect(state.pending.map((each) => each.key)).toEqual(['q:1'])
    expect(restoreButtons(mounted.root, 'match-league')[0].props.disabled).toBe(true)
    state.pending[0].finish()
    await mounted.settle()
    expect(restoreButtons(mounted.root, 'match-league')[0].props.disabled).toBe(false)
  })

  it('restores a league by its run key, busy on that row only', async () => {
    state.view.leagues.push({ runKey: 'q:2', name: 'Qother', startedAtMs: OCT_11, removedAtMs: OCT_11, wins: 0, losses: 0, deckName: null, format: null })
    const mounted = mountFake(ArchivesPage)
    click(triggers(mounted.root)[1])
    await mounted.settle()
    click(restoreButtons(mounted.root, 'league')[0])
    await mounted.settle()
    expect(state.pending.map((each) => each.key)).toEqual(['q:1'])
    expect(restoreButtons(mounted.root, 'league').map((node) => node.props.disabled)).toEqual([true, false])
    state.pending[0].finish()
    await mounted.settle()
    expect(restoreButtons(mounted.root, 'league').map((node) => node.props.disabled)).toEqual([false, false])
  })

  it('says so when a tab is empty', async () => {
    state.view = { leagues: [], matches: [] }
    const mounted = mountFake(ArchivesPage)
    expect(textOf(mounted.root)).toContain('No deleted matches')
    expect(restoreButtons(mounted.root, 'match')).toEqual([])
    click(triggers(mounted.root)[1])
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('No deleted leagues')
    expect(triggers(mounted.root).map((node) => textOf(node).trim())).toEqual(['Matches (0)', 'Leagues (0)'])
  })
})
