import { describe, expect, it, vi } from 'vitest'
import { findAll, mountFake } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ calls: [] as unknown[][], statuses: [] as unknown[][] }))
// Each dialog records how it was opened; the menu keeps its props and listeners.
const dialog = vi.hoisted(() => async (name: string) => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      inheritAttrs: false,
      setup(_, { expose, attrs }) {
        expose({ openForRun: (...args: unknown[]) => state.calls.push([name, ...args]) })
        return () => h(name, { ...attrs })
      },
    }),
  }
})

vi.mock('@/components/leagues/DeleteLeagueDialog.vue', () => dialog('delete-dialog'))
vi.mock('@/components/leagues/DropLeagueDialog.vue', () => dialog('drop-dialog'))
vi.mock('@/components/leagues/LeagueNotes.vue', () => dialog('league-notes'))
vi.mock('@/components/leagues/LinkMatchesDialog.vue', () => dialog('link-dialog'))
vi.mock('@/components/leagues/LeagueContextMenu.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: true, props: { canDelete: Boolean, canAddMatches: Boolean }, setup: (p) => () => h('league-menu', { ...p }) }) }
})
vi.mock('@/composables/useLeagueManage', () => ({
  useLeagueManage: () => ({
    setStatus: async (...args: unknown[]) => {
      state.statuses.push(args)
      return true
    },
  }),
}))

const { default: LeagueRunActions } = await import('./LeagueRunActions.vue')

const BASE = {
  runKey: 'q-draft:2',
  matchCount: 3,
  manual: false,
  limited: true,
  userState: null,
  autoState: 'complete',
  notes: 'a note',
}

function menu(props: Record<string, unknown> = {}) {
  const mounted = mountFake(LeagueRunActions, { ...BASE, ...props })
  expect(mounted.warnings).toEqual([])
  const [node] = findAll(mounted.root, (candidate) => candidate.tag === 'league-menu')
  return node
}

function call(node: { props: Record<string, unknown> }, name: string, ...args: unknown[]) {
  ;(node.props[name] as (...rest: unknown[]) => void)(...args)
}

describe('the league actions', () => {
  it('manages a limited run: status, delete once not Active, and Add matches', () => {
    const node = menu()
    expect(node.props.canDelete).toBe(true)
    expect(node.props.canAddMatches).toBe(true)
    expect(menu({ autoState: 'active' }).props.canDelete).toBe(false)
    // The player's status wins over the fold's.
    expect(menu({ autoState: 'active', userState: 'dropped' }).props.canDelete).toBe(true)
    expect(menu({ autoState: 'complete', userState: 'active' }).props.canDelete).toBe(false)
  })

  it('tells the Drop dialog the platform (MTGO unless named)', () => {
    const platformOf = (props: Record<string, unknown>) => {
      const mounted = mountFake(LeagueRunActions, { ...BASE, ...props })
      const [drop] = findAll(mounted.root, (candidate) => candidate.tag === 'drop-dialog')
      return drop.props.platform
    }
    expect(platformOf({})).toBe('mtgo')
    expect(platformOf({ platform: 'arena' })).toBe('arena')
  })

  it('caps a manual league at five matches', () => {
    expect(menu({ manual: true, limited: false, matchCount: 5 }).props.canAddMatches).toBe(false)
    expect(menu({ manual: true, limited: false, matchCount: 4 }).props.canAddMatches).toBe(true)
  })

  it('asks before Dropped, and sets any other status at once', () => {
    state.calls = []
    state.statuses = []
    const node = menu()
    call(node, 'onSetStatus', 'dropped')
    call(node, 'onSetStatus', 'partial')
    call(node, 'onSetStatus', null)
    expect(state.calls).toEqual([['drop-dialog', 'q-draft:2']])
    expect(state.statuses).toEqual([
      ['q-draft:2', 'partial'],
      ['q-draft:2', null],
    ])
  })

  it('opens Delete with the match count, notes with the notes, and Link with the limited flag', () => {
    state.calls = []
    const node = menu()
    call(node, 'onDelete')
    call(node, 'onEditNotes')
    call(node, 'onAddMatches')
    expect(state.calls).toEqual([
      ['delete-dialog', 'q-draft:2', 3],
      ['league-notes', 'q-draft:2', 'a note'],
      ['link-dialog', 'q-draft:2', 3, false, true],
    ])
  })
})
