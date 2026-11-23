import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest'
import { defineComponent, h, provide } from 'vue'
import { replayContextKey, type ReplayContext } from './replayContext'
import { findAll, hasClass, htmlText, mountFake, renderHtml, textOf, type FakeNode } from './replayTestHarness'
import type { ReplayFrame, ReplayMatchGame } from './types'

// The replay window's Tauri window: fullscreen as the compositor grants it.
const tauriWindow = vi.hoisted(() => {
  const state = {
    fullscreen: false,
    /** A compositor that refuses fullscreen. */
    refuse: false,
    resized: [] as (() => void)[],
    focused: [] as (() => void)[],
    setFullscreen: async (on: boolean) => {
      if (!state.refuse) state.fullscreen = on
    },
    isFullscreen: async () => state.fullscreen,
    onResized: async (handler: () => void) => {
      state.resized.push(handler)
      return () => {
        state.resized = state.resized.filter((item) => item !== handler)
      }
    },
    onFocusChanged: async (handler: () => void) => {
      state.focused.push(handler)
      return () => {
        state.focused = state.focused.filter((item) => item !== handler)
      }
    },
  }
  return state
})
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => tauriWindow }))
vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string, scheme: string) => `http://${scheme}.localhost/${encodeURIComponent(path)}`,
}))

const { default: ReplayViewer } = await import('./ReplayViewer.vue')
const { default: ReplayGameEnd } = await import('./ReplayGameEnd.vue')
const { default: ReplayLogDrawer } = await import('./ReplayLogDrawer.vue')
const { default: ReplayCardImage } = await import('./ReplayCardImage.vue')
const { useReplayFullscreen } = await import('./useReplayFullscreen')
const { useReplayKeyboard } = await import('./useReplayKeyboard')

const fixtures = import.meta.glob<ReplayFrame[]>('./__fixtures__/repeats-dropped.json', { eager: true, import: 'default' })

/** V4's fixture game as the window gets it: the local seat marked, cards named, a local image file. */
function fixtureFrames(): ReplayFrame[] {
  const frames: ReplayFrame[] = structuredClone(fixtures['./__fixtures__/repeats-dropped.json'])
  for (const frame of frames) {
    for (const player of frame.content.Players) player.IsLocal = player.Name === 'Qplayer'
    for (const card of frame.content.Cards) {
      Object.assign(card, { name: 'Qcard', type: 'Creature', image: 'https://cards.example/q.jpg', local_image: 'q.jpg' })
    }
  }
  return frames
}

const threeGames: ReplayMatchGame[] = [
  { id: 11, number: 1, won: true },
  { id: 12, number: 2, won: false },
  { id: 13, number: 3, won: null },
]

function viewerProps(overrides: Record<string, unknown> = {}) {
  return {
    frames: fixtureFrames(),
    log: [],
    won: true,
    gameId: 12,
    matchGames: threeGames,
    sideboard: null,
    previousSideboard: null,
    archetypes: null,
    onSelectGame: () => {},
    ...overrides,
  }
}

/** The context ReplayViewer provides, for rendering one of its parts alone. */
function inContext(component: unknown, props: Record<string, unknown>, selectGame: ((id: number) => void) | null) {
  const context: ReplayContext = {
    showPreview: () => {},
    hidePreview: () => {},
    pinPreview: () => {},
    gameHref: null,
    linkComponent: 'a',
    selectGame,
    archetype: () => null,
  }
  return defineComponent({
    setup() {
      provide(replayContextKey, context)
      return () => h(component as never, props)
    },
  })
}

// The keyboard listens on the window and checks the event target and open dialogs.
class FakeElement {
  constructor(private readonly editable: boolean) {}
  closest() {
    return this.editable ? this : null
  }
}
let keydown: ((event: unknown) => void) | null = null
let dialogOpen = false

function press(key: string, options: { shiftKey?: boolean; target?: unknown } = {}) {
  const event = { key, shiftKey: options.shiftKey ?? false, target: options.target ?? null, preventDefault: vi.fn() }
  keydown?.(event)
  return event
}

beforeEach(() => {
  tauriWindow.fullscreen = false
  tauriWindow.refuse = false
  tauriWindow.resized = []
  tauriWindow.focused = []
  keydown = null
  dialogOpen = false
  vi.stubGlobal('window', {
    addEventListener: (type: string, handler: (event: unknown) => void) => {
      if (type === 'keydown') keydown = handler
    },
    removeEventListener: (type: string) => {
      if (type === 'keydown') keydown = null
    },
    localStorage: { getItem: () => null, setItem: () => {} },
  })
  vi.stubGlobal('document', { querySelector: () => (dialogOpen ? {} : null) })
  vi.stubGlobal('Element', FakeElement)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function buttonTitled(root: FakeNode, title: string): FakeNode | undefined {
  return findAll(root, (node) => node.tag === 'button' && node.props.title === title)[0]
}

describe('replay viewer', () => {
  test('the empty state strings', async () => {
    const { html, warnings } = await renderHtml(ReplayViewer, viewerProps({ frames: [] }))
    expect(htmlText(html)).toContain('No frames recorded')
    expect(htmlText(html)).toContain('This game was saved without any game states, so there is nothing to replay.')
    expect(warnings).toEqual([])
  })

  test('the game picker is hidden for one game and shown as buttons for three', async () => {
    const one = await renderHtml(ReplayViewer, viewerProps({ matchGames: [threeGames[1]] }))
    expect(one.html).not.toContain('aria-label="Games in this match"')

    const three = await renderHtml(ReplayViewer, viewerProps())
    const picker = three.html.match(/<nav aria-label="Games in this match"[\s\S]*?<\/nav>/)?.[0] ?? ''
    expect(htmlText(picker)).toBe('G1 G2 G3')
    expect(picker.match(/<button/g)?.length).toBe(3)
    expect(picker).toContain('aria-current="page"')
    expect(three.warnings).toEqual([])
  })

  test('the picker switches games in place', async () => {
    const selected: number[] = []
    const mounted = mountFake(ReplayViewer, viewerProps({ onSelectGame: (id: number) => selected.push(id) }))
    await mounted.settle()
    const game3 = buttonTitled(mounted.root, 'Game 3')!
    ;(game3.props.onClick as () => void)()
    expect(selected).toEqual([13])
    mounted.unmount()
  })

  test('the game end panel strings for won, lost and unknown', async () => {
    const cases: [boolean | null, string][] = [
      [true, 'You won'],
      [false, 'You lost'],
      [null, 'Result not recorded'],
    ]
    for (const [won, text] of cases) {
      const panel = inContext(ReplayGameEnd, { game: threeGames[1], next: threeGames[2], won }, () => {})
      const { html, warnings } = await renderHtml(panel)
      expect(htmlText(html)).toBe(`Game 2 complete ${text} Watch again Watch game 3`)
      expect(warnings).toEqual([])
    }
    const last = inContext(ReplayGameEnd, { game: threeGames[2], next: null, won: null }, () => {})
    expect(htmlText((await renderHtml(last)).html)).toBe('Game 3 complete Result not recorded Watch again')
  })

  test('the log drawer empty text', async () => {
    const { html } = await renderHtml(ReplayLogDrawer, {
      open: true,
      items: [],
      current: 0,
      localId: 1,
      playerName: () => 'Qplayer',
    })
    expect(htmlText(html)).toContain('Game log 0 events')
    expect(htmlText(html)).toContain('No game log was recorded for this game.')
  })

  test('a 3-game view mounts without warnings', async () => {
    const mounted = mountFake(ReplayViewer, viewerProps())
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('Qplayer')
    expect(textOf(mounted.root)).toContain('1 / 6')
    expect(buttonTitled(mounted.root, 'Fullscreen (F)')).toBeDefined()
    mounted.unmount()
    expect(mounted.warnings).toEqual([])
  })

  test('card images load the local file through the card scheme, else the remote one, else the gradient', async () => {
    const local = await renderHtml(ReplayCardImage, { name: 'Qcard', image: 'https://cards.example/q.jpg', local: 'q.jpg' })
    expect(local.html).toContain('src="http://card.localhost/q.jpg"')

    const remote = await renderHtml(ReplayCardImage, { name: 'Qcard', image: 'https://cards.example/q.jpg', local: null })
    expect(remote.html).toContain('src="https://cards.example/q.jpg"')

    const none = await renderHtml(ReplayCardImage, { name: null, image: null })
    expect(none.html).not.toContain('<img')
    expect(htmlText(none.html)).toBe('Unknown card')

    // A missing file falls back to the remote image, and a failing remote image to the gradient.
    const mounted = mountFake(ReplayCardImage, { name: 'Qcard', image: 'https://cards.example/q.jpg', local: 'q.jpg' })
    const img = () => findAll(mounted.root, (node) => node.tag === 'img')[0]
    ;(img().props.onError as () => void)()
    await mounted.settle()
    expect(img().props.src).toBe('https://cards.example/q.jpg')
    ;(img().props.onError as () => void)()
    await mounted.settle()
    expect(img()).toBeUndefined()
    expect(textOf(mounted.root)).toContain('Qcard')
  })

  test('the viewer passes local images to its cards', async () => {
    const mounted = mountFake(ReplayViewer, viewerProps())
    await mounted.settle()
    press('ArrowRight')
    await mounted.settle()
    const images = findAll(mounted.root, (node) => node.tag === 'img').map((node) => node.props.src)
    expect(images).toContain('http://card.localhost/q.jpg')
    mounted.unmount()
  })
})

describe('replay keyboard', () => {
  function keyboard() {
    const actions = {
      toggle: vi.fn(),
      step: vi.fn(),
      jumpTurn: vi.fn(),
      toggleLog: vi.fn(),
      toggleFullscreen: vi.fn(),
      dismiss: vi.fn(),
    }
    const mounted = mountFake(
      defineComponent({
        setup() {
          useReplayKeyboard(actions)
          return () => null
        },
      }),
    )
    return { actions, mounted }
  }

  test('the keyboard map', () => {
    const { actions, mounted } = keyboard()
    expect(press(' ').preventDefault).toHaveBeenCalled()
    expect(actions.toggle).toHaveBeenCalledTimes(1)
    press('ArrowRight')
    press('ArrowLeft')
    expect(actions.step.mock.calls).toEqual([[1], [-1]])
    press('ArrowRight', { shiftKey: true })
    press('ArrowLeft', { shiftKey: true })
    expect(actions.jumpTurn.mock.calls).toEqual([[1], [-1]])
    press('l')
    press('L')
    expect(actions.toggleLog).toHaveBeenCalledTimes(2)
    press('f')
    press('F')
    expect(actions.toggleFullscreen).toHaveBeenCalledTimes(2)
    press('Escape')
    expect(actions.dismiss).toHaveBeenCalledTimes(1)
    press('x')
    mounted.unmount()
    expect(keydown).toBeNull()
  })

  test('keys are ignored in an input and with a dialog open', () => {
    const { actions, mounted } = keyboard()
    press(' ', { target: new FakeElement(true) })
    press('ArrowRight', { target: new FakeElement(true) })
    dialogOpen = true
    press(' ')
    press('Escape')
    press('f')
    expect(actions.toggle).not.toHaveBeenCalled()
    expect(actions.step).not.toHaveBeenCalled()
    expect(actions.dismiss).not.toHaveBeenCalled()
    expect(actions.toggleFullscreen).not.toHaveBeenCalled()
    dialogOpen = false
    press(' ', { target: new FakeElement(false) })
    expect(actions.toggle).toHaveBeenCalledTimes(1)
    mounted.unmount()
  })
})

describe('replay fullscreen', () => {
  function fullscreen() {
    let state: ReturnType<typeof useReplayFullscreen> | null = null
    const mounted = mountFake(
      defineComponent({
        setup() {
          state = useReplayFullscreen()
          return () => null
        },
      }),
    )
    return { state: state!, mounted }
  }

  test('fullscreen calls the window API', async () => {
    const setFullscreen = vi.spyOn(tauriWindow, 'setFullscreen')
    const { state, mounted } = fullscreen()
    await mounted.settle()
    expect(state.supported.value).toBe(true)
    expect(state.active.value).toBe(false)

    await state.toggle()
    expect(setFullscreen).toHaveBeenLastCalledWith(true)
    expect(state.active.value).toBe(true)

    await state.toggle()
    expect(setFullscreen).toHaveBeenLastCalledWith(false)
    expect(state.active.value).toBe(false)
    mounted.unmount()
    expect(tauriWindow.resized).toEqual([])
    expect(tauriWindow.focused).toEqual([])
    setFullscreen.mockRestore()
  })

  test('fullscreen_follows_the_window_not_the_request', async () => {
    // A window that refuses fullscreen leaves the button off.
    tauriWindow.refuse = true
    const refused = fullscreen()
    await refused.mounted.settle()
    await refused.state.toggle()
    expect(refused.state.active.value).toBe(false)
    refused.mounted.unmount()

    // One that leaves fullscreen on its own turns it off after the resize event.
    tauriWindow.refuse = false
    const left = fullscreen()
    await left.mounted.settle()
    await left.state.toggle()
    expect(left.state.active.value).toBe(true)
    tauriWindow.fullscreen = false
    tauriWindow.resized.forEach((handler) => handler())
    await left.mounted.settle()
    expect(left.state.active.value).toBe(false)

    // Fullscreen granted from outside shows on focus.
    tauriWindow.fullscreen = true
    tauriWindow.focused.forEach((handler) => handler())
    await left.mounted.settle()
    expect(left.state.active.value).toBe(true)
    left.mounted.unmount()
  })

  test('escape_leaves_fullscreen_before_dismissing', async () => {
    const mounted = mountFake(ReplayViewer, viewerProps())
    await mounted.settle()
    press('ArrowRight')
    await mounted.settle()

    // Pin the preview with a tap on the card in play.
    const tile = findAll(mounted.root, (node) => typeof node.props.onMouseenter === 'function' && typeof node.props.onClick === 'function')[0]
    ;(tile.props.onClick as (event: unknown) => void)({ currentTarget: tile, stopPropagation: () => {} })
    await mounted.settle()
    const pinned = () => findAll(mounted.root, (node) => hasClass(node, 'z-89')).length > 0
    expect(pinned()).toBe(true)

    press('f')
    await mounted.settle()
    expect(tauriWindow.fullscreen).toBe(true)
    expect(buttonTitled(mounted.root, 'Exit fullscreen (F)')).toBeDefined()

    press('Escape')
    await mounted.settle()
    expect(tauriWindow.fullscreen).toBe(false)
    expect(buttonTitled(mounted.root, 'Fullscreen (F)')).toBeDefined()
    expect(pinned()).toBe(true)

    press('Escape')
    await mounted.settle()
    expect(pinned()).toBe(false)
    mounted.unmount()
    expect(mounted.warnings).toEqual([])
  })
})
