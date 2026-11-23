import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ImportProgress, WhatsNewAnswer } from '@/bindings'

const state = vi.hoisted(() => ({
  answers: [] as WhatsNewAnswer[],
  asked: [] as string[][],
  fail: false,
  // Runs while the command answers, before the answer reaches the window.
  duringAsk: null as null | (() => void),
  importListeners: [] as ((event: { payload: { progress: ImportProgress } }) => void)[],
  settingsListeners: [] as (() => void)[],
}))

vi.mock('@/bindings', () => ({
  commands: {
    whatsNewOnStart: async (content: string[]) => {
      state.asked.push(content)
      const during = state.duringAsk
      state.duringAsk = null
      during?.()
      if (state.fail) return { status: 'error', error: { kind: 'notReady', message: 'no' } }
      return { status: 'ok', data: state.answers.shift() ?? 'nothing' }
    },
  },
  events: {
    importProgressChanged: {
      listen: async (handler: (event: { payload: { progress: ImportProgress } }) => void) => {
        state.importListeners.push(handler)
        return () => (state.importListeners = state.importListeners.filter((h) => h !== handler))
      },
    },
    settingsChanged: {
      listen: async (handler: () => void) => {
        state.settingsListeners.push(handler)
        return () => (state.settingsListeners = state.settingsListeners.filter((h) => h !== handler))
      },
    },
  },
}))
vi.mock('@/lib/queries', () => ({ useAppInfo: () => ({ data: { value: null } }) }))

const { askWhatsNewOnStart } = await import('./useWhatsNew')
const { whatsNewVersions, whatsNewEntries } = await import('@/content/whats-new')

function fakeRouter(name = 'dashboard') {
  const pushed: unknown[] = []
  return {
    pushed,
    router: {
      isReady: async () => {},
      currentRoute: { value: { name } },
      push: async (to: unknown) => {
        pushed.push(to)
      },
    },
  }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

beforeEach(() => {
  state.answers = []
  state.asked = []
  state.fail = false
  state.duringAsk = null
  state.importListeners = []
  state.settingsListeners = []
})

describe('What\'s new after start', () => {
  // RedirectToWhatsNewTest "redirects once on a minor bump": the main window asks once.
  it('passes the entries and opens the page when the answer is show', async () => {
    state.answers = ['show']
    const { router, pushed } = fakeRouter()
    await askWhatsNewOnStart(router)
    expect(state.asked).toEqual([whatsNewVersions(whatsNewEntries)])
    expect(pushed).toEqual([{ name: 'whats-new' }])
  })

  it('stays put when the answer is nothing, or the command fails', async () => {
    state.answers = ['nothing']
    const { router, pushed } = fakeRouter()
    await askWhatsNewOnStart(router)
    state.fail = true
    await askWhatsNewOnStart(router)
    expect(pushed).toEqual([])
  })

  // "does not redirect the page to itself".
  it('never opens the page on top of itself', async () => {
    state.answers = ['show']
    const { router, pushed } = fakeRouter('whats-new')
    await askWhatsNewOnStart(router)
    expect(pushed).toEqual([])
  })

  // Spec section 7: never over the first-run choice or the 0.x import progress.
  it('asks again once the import is over when told to wait', async () => {
    state.answers = ['wait', 'show']
    const { router, pushed } = fakeRouter()
    const asking = askWhatsNewOnStart(router)
    await flush()
    expect(pushed).toEqual([])
    state.importListeners.forEach((listen) => listen({ payload: { progress: { phase: 'reading' } as ImportProgress } }))
    await flush()
    expect(state.asked).toHaveLength(1)
    state.importListeners.forEach((listen) =>
      listen({ payload: { progress: { phase: 'failed', message: 'x' } as ImportProgress } }),
    )
    await asking
    expect(state.asked).toHaveLength(2)
    expect(pushed).toEqual([{ name: 'whats-new' }])
    expect(state.importListeners).toEqual([])
    expect(state.settingsListeners).toEqual([])
  })

  it('asks again when the settings change while waiting (the first-run choice)', async () => {
    state.answers = ['wait', 'nothing']
    const { router } = fakeRouter()
    const asking = askWhatsNewOnStart(router)
    await flush()
    state.settingsListeners.forEach((listen) => listen())
    await asking
    expect(state.asked).toHaveLength(2)
    expect(state.settingsListeners).toEqual([])
  })

  // W5 review Minor 1: the import may finish (or the choice be saved) after
  // the app answers wait but before the window listens; that wake-up is not lost.
  it('asks again when the import ends while the wait answer is on its way', async () => {
    state.answers = ['wait', 'show']
    state.duringAsk = () =>
      state.importListeners.forEach((listen) =>
        listen({ payload: { progress: { phase: 'finished' } as ImportProgress } }),
      )
    const { router, pushed } = fakeRouter()
    const asking = askWhatsNewOnStart(router)
    await flush()
    await flush()
    expect(state.asked).toHaveLength(2)
    await asking
    expect(pushed).toEqual([{ name: 'whats-new' }])
    expect(state.importListeners).toEqual([])
    expect(state.settingsListeners).toEqual([])
  })

  it('asks again when the first-run choice is saved while the wait answer is on its way', async () => {
    state.answers = ['wait', 'show']
    state.duringAsk = () => state.settingsListeners.forEach((listen) => listen())
    const { router, pushed } = fakeRouter()
    const asking = askWhatsNewOnStart(router)
    await flush()
    await flush()
    expect(state.asked).toHaveLength(2)
    await asking
    expect(pushed).toEqual([{ name: 'whats-new' }])
  })

  it('stops listening when the first answer is final', async () => {
    state.answers = ['nothing']
    const { router } = fakeRouter()
    await askWhatsNewOnStart(router)
    expect(state.importListeners).toEqual([])
    expect(state.settingsListeners).toEqual([])
  })
})
