import type { VNodeArrayChildren } from 'vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import lossRecord from '../../../crates/arena-game/src/snapshots/arena_game__mtgr__tests__loss_record.snap?raw'
import { findAll, mountFake, textOf, type FakeNode } from './replayTestHarness'

const bindings = vi.hoisted(() => ({
  commands: {
    replayView: vi.fn(),
    replayRequestedGame: vi.fn(async () => null),
  },
  events: { replayGameChanged: { listen: vi.fn(async (_handler?: unknown) => () => {}) } },
}))
vi.mock('@/bindings', () => bindings)
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string, scheme: string) => `http://${scheme}.localhost/${encodeURIComponent(path)}`,
}))

// The MTGO viewer as a stub that renders its `actions` slot, and the share button as a marker.
vi.mock('./ReplayViewer.vue', async () => {
  const { h } = await import('vue')
  return {
    default: {
      setup: (_: unknown, { slots }: { slots: Record<string, (() => VNodeArrayChildren) | undefined> }) =>
        () => h('div', slots.actions?.()),
    },
  }
})
vi.mock('./share/ReplayShareButton.vue', async () => {
  const { h } = await import('vue')
  return { default: { setup: () => () => h('span', 'SHARE_BUTTON') } }
})

/** The `.mtgr` text after the insta header of the three-game loss golden. */
function goldenText(): string {
  const lines = lossRecord.split('\n')
  let seen = 0
  for (const [i, line] of lines.entries()) {
    if (line === '---' && ++seen === 2) return lines.slice(i + 1).join('\n')
  }
  throw new Error('snapshot has no insta header')
}

const { default: ReplayPage } = await import('./ReplayPage.vue')

beforeEach(() => {
  const listeners = { addEventListener: () => {}, removeEventListener: () => {} }
  vi.stubGlobal('window', { ...listeners, localStorage: { getItem: () => null, setItem: () => {} } })
  vi.stubGlobal('document', { ...listeners, querySelector: () => null, fullscreenElement: null })
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.clearAllMocks()
})

describe('the replay page', () => {
  it('a game with no replay shows the empty state, not a blank page', async () => {
    bindings.commands.replayView.mockResolvedValue({ status: 'ok', data: null })
    const mounted = mountFake(ReplayPage, { initialGame: 7 })
    await mounted.settle()
    expect(mounted.warnings).toEqual([])
    const shown = textOf(mounted.root)
    expect(shown).toContain('No replay to show')
    expect(shown).toContain('This game is no longer stored.')
    expect(shown).not.toContain('No frames recorded')
    mounted.unmount()
  })

  it('a stored game with no record says so', async () => {
    bindings.commands.replayView.mockResolvedValue({ status: 'ok', data: { kind: 'noRecord' } })
    const mounted = mountFake(ReplayPage, { initialGame: 7 })
    await mounted.settle()
    expect(mounted.warnings).toEqual([])
    const shown = textOf(mounted.root)
    expect(shown).toContain('No replay was recorded for this game.')
    expect(shown).not.toContain('no longer stored')
    mounted.unmount()
  })

  it('a failed read still shows its error', async () => {
    bindings.commands.replayView.mockResolvedValue({ status: 'error', error: 'The database is busy.' })
    const mounted = mountFake(ReplayPage, { initialGame: 7 })
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('The database is busy.')
    mounted.unmount()
  })

  it('shows the share button only for a shareable snapshot', async () => {
    const snapshot = (shareable: boolean) => ({
      status: 'ok',
      data: {
        kind: 'snapshot',
        gameId: 7,
        frames: [],
        log: [],
        won: null,
        matchGames: [],
        sideboard: null,
        previousSideboard: null,
        archetypes: [],
        share: { linked: true, supporter: true, url: null, shareable },
      },
    })
    bindings.commands.replayView.mockResolvedValue(snapshot(true))
    const shareable = mountFake(ReplayPage, { initialGame: 7 })
    await shareable.settle()
    expect(textOf(shareable.root)).toContain('SHARE_BUTTON')
    shareable.unmount()

    bindings.commands.replayView.mockResolvedValue(snapshot(false))
    const notShareable = mountFake(ReplayPage, { initialGame: 7 })
    await notShareable.settle()
    expect(notShareable.warnings).toEqual([])
    expect(textOf(notShareable.root)).not.toContain('SHARE_BUTTON')
    notShareable.unmount()
  })

  it('opens the first game again after the in-window picker moved to another', async () => {
    const mtgr = { kind: 'mtgr', text: goldenText(), gameIndex: 0, cards: [], printings: [] }
    // A fresh view object per read, as the command returns.
    bindings.commands.replayView.mockImplementation(async () => ({ status: 'ok', data: { ...mtgr } }))
    let changed: (event: { payload: { gameId: number } }) => void = () => {}
    bindings.events.replayGameChanged.listen.mockImplementation(async (handler?: unknown) => {
      changed = handler as typeof changed
      return () => {}
    })
    const mounted = mountFake(ReplayPage, { initialGame: 7 })
    await mounted.settle()
    expect(bindings.commands.replayView).toHaveBeenCalledTimes(1)
    const current = (node: FakeNode) => findAll(node, (item) => item.props['aria-current'] === 'page').map((item) => item.props.title)
    expect(current(mounted.root)).toEqual(['Game 1'])

    // The picker shows game 3 in place.
    const game3 = findAll(mounted.root, (node) => node.props.title === 'Game 3')[0]
    ;(game3.props.onClick as () => void)()
    await mounted.settle()
    expect(current(mounted.root)).toEqual(['Game 3'])
    expect(bindings.commands.replayView).toHaveBeenCalledTimes(1)

    // The app asks for game row 7 again: it opens, game 1 shows.
    changed({ payload: { gameId: 7 } })
    await mounted.settle()
    expect(bindings.commands.replayView).toHaveBeenCalledTimes(2)
    expect(current(mounted.root)).toEqual(['Game 1'])
    mounted.unmount()
  })
})
