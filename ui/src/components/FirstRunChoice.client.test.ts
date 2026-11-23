import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import type { FirstRunChoice, FirstRunState, ImportProgress } from '@/bindings'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  firstRun: { decision: 'ask', error: null } as FirstRunState,
  progress: null as ImportProgress | null,
  running: false,
  chosen: [] as FirstRunChoice[],
  stored: new Map<string, unknown>(),
  invalidated: [] as string[],
  dismissed: 0,
  startFails: false,
  path: '/',
}))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub, useRoute: () => ({ path: state.path }) }
})
vi.mock('@/bindings', () => ({
  commands: {
    dismissFirstRun: async () => {
      state.dismissed += 1
      return { status: 'ok', data: { decision: 'nothing', error: null } }
    },
    chooseFirstRun: async (choice: FirstRunChoice) => {
      state.chosen.push(choice)
      if (state.startFails) return { status: 'error', error: { Storage: 'no MyMTGO 0.x database was found' } }
      const decision = choice === 'migrate' ? 'migrate' : 'fresh'
      return { status: 'ok', data: { decision, error: null } }
    },
  },
}))
vi.mock('@/queryClient', () => ({
  queryClient: {
    setQueryData: (key: readonly string[], value: unknown) => {
      state.stored.set(key.join('/'), value)
    },
    invalidateQueries: ({ queryKey }: { queryKey: readonly string[] }) => {
      state.invalidated.push(queryKey.join('/'))
    },
  },
}))
vi.mock('@/lib/queries', () => ({
  useFirstRun: () => ({ data: ref(state.firstRun) }),
  useLegacyImportProgress: () => ({ data: ref(state.progress) }),
  useLegacyImport: () => ({
    data: ref({ detectedPath: 'Q.sqlite', chosenPath: null, lastImport: null, running: state.running }),
  }),
}))

const { default: FirstRunChoiceView } = await import('./FirstRunChoice.vue')

function mount() {
  const mounted = mountFake(FirstRunChoiceView)
  expect(mounted.warnings).toEqual([])
  return mounted
}

function button(root: FakeNode, label: string): FakeNode {
  const found = findAll(root, (node) => node.tag === 'button').find((node) => textOf(node).trim() === label)
  if (found === undefined) throw new Error(`no ${label} button in ${textOf(root)}`)
  return found
}

beforeEach(() => {
  state.firstRun = { decision: 'ask', error: null }
  state.progress = null
  state.running = false
  state.chosen = []
  state.stored = new Map()
  state.invalidated = []
  state.dismissed = 0
  state.startFails = false
  state.path = '/'
})

function page() {
  return mountFake(
    defineComponent({ render: () => h(FirstRunChoiceView, null, { default: () => 'the page' }) }),
  )
}

// Release spec section 13.2 steps 3 to 5.
describe('the first-run 0.x choice', () => {
  it('asks with the spec strings', () => {
    const text = textOf(mount().root)
    expect(text).toContain('MyMTGO found data from MyMTGO 0.x on this computer.')
    expect(text).toContain('Migrate from MyMTGO 0.x')
    expect(text).toContain('Start fresh')
  })

  it('starts the import on "Migrate from MyMTGO 0.x" and shows its progress', async () => {
    const mounted = mount()
    ;(button(mounted.root, 'Migrate from MyMTGO 0.x').props.onClick as () => void)()
    await mounted.settle()
    expect(state.chosen).toEqual(['migrate'])
    expect(state.stored.get('legacyImportProgress')).toEqual({ phase: 'reading' })
    expect(state.stored.get('firstRun')).toEqual({ decision: 'migrate', error: null })

    state.firstRun = { decision: 'migrate', error: null }
    state.progress = { phase: 'writing', done: 2, total: 5 }
    const text = textOf(mount().root)
    expect(text).toContain('Importing 2 of 5 matches')
    expect(findAll(mount().root, (node) => node.tag === 'button')).toEqual([])
  })

  it('shows the normal first run on "Start fresh"', async () => {
    const mounted = mount()
    ;(button(mounted.root, 'Start fresh').props.onClick as () => void)()
    await mounted.settle()
    expect(state.chosen).toEqual(['fresh'])
    expect(state.stored.get('firstRun')).toEqual({ decision: 'fresh', error: null })
    expect(state.stored.has('legacyImportProgress')).toBe(false)

    // Settled: the page itself shows (the slot), not the question.
    state.firstRun = { decision: 'fresh', error: null }
    const page = mountFake(
      defineComponent({ render: () => h(FirstRunChoiceView, null, { default: () => 'the normal first run' }) }),
    )
    expect(textOf(page.root)).toBe('the normal first run')
  })

  it('shows a failed migration with "Try again" and the Import page', async () => {
    state.firstRun = { decision: 'migrate', error: null }
    state.progress = { phase: 'failed', message: 'the 0.x data could not be read' }
    const mounted = mount()
    const text = textOf(mounted.root)
    expect(text).toContain('the 0.x data could not be read')
    expect(findAll(mounted.root, (node) => node.tag === 'a' && node.props.href === '/import')).toHaveLength(1)
    ;(button(mounted.root, 'Try again').props.onClick as () => void)()
    await mounted.settle()
    expect(state.chosen).toEqual(['migrate'])
  })

  it('shows the Import page itself under a failed migration', () => {
    state.firstRun = { decision: 'migrate', error: 'no MyMTGO 0.x database was found; choose it on the Import page' }
    expect(textOf(page().root)).toContain('Try again')
    state.path = '/import'
    expect(textOf(page().root)).toBe('the page')
  })

  it('keeps the import screen on every page while the migration runs', () => {
    state.firstRun = { decision: 'migrate', error: null }
    state.running = true
    state.path = '/import'
    expect(textOf(page().root)).toContain('Importing from MyMTGO 0.x')
  })

  it('lets a failed migration be left for this run', async () => {
    state.firstRun = { decision: 'migrate', error: 'the 0.x data could not be read' }
    const mounted = page()
    ;(button(mounted.root, 'Continue without importing').props.onClick as () => void)()
    await mounted.settle()
    expect(state.dismissed).toBe(1)
    expect(state.stored.get('firstRun')).toEqual({ decision: 'nothing', error: null })
    expect(state.chosen).toEqual([])
  })

  it('refetches the first-run state when the migration could not start', async () => {
    state.startFails = true
    const mounted = mount()
    ;(button(mounted.root, 'Migrate from MyMTGO 0.x').props.onClick as () => void)()
    await mounted.settle()
    expect(state.invalidated).toEqual(['firstRun'])
    expect(state.stored.get('legacyImportProgress')).toBeNull()
  })
})
