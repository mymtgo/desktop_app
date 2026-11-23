import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { AccountsView, PlatformContextView } from '@/bindings'
import { hasFeature } from '@/lib/platformFeatures'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  view: { accounts: [], current: null } as AccountsView,
  switched: [] as string[],
  pushed: [] as string[],
  path: '/',
  context: { current: 'mtgo', mtgo: true, arena: false, switchable: false } as PlatformContextView,
  switchedPlatforms: [] as string[],
  info: { productName: 'Qapp', prerelease: true } as { productName: string; prerelease: boolean } | null,
}))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return {
    RouterLink: RouterLinkStub,
    useRoute: () => ({ path: state.path }),
    useRouter: () => ({
      push: async (to: string) => {
        state.pushed.push(to)
      },
    }),
  }
})
vi.mock('@/bindings', () => ({
  commands: {
    switchAccount: async (username: string) => {
      state.switched.push(username)
      return { status: 'ok', data: { redirect: '/decks' } }
    },
  },
}))
vi.mock('@/lib/queries', () => ({
  useAccounts: () => ({ data: ref(state.view) }),
  useAppInfo: () => ({ data: ref(state.info) }),
}))
vi.mock('@/composables/usePlatformContext', () => ({
  usePlatformContext: () => ({
    context: ref(state.context),
    hasFeature: (feature: Parameters<typeof hasFeature>[1]) => hasFeature(state.context.current, feature),
    livePlatforms: ref([]),
    switchTo: async (platform: string) => {
      state.switchedPlatforms.push(platform)
    },
  }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))
// The menu as plain elements: the fake renderer has no portal or popper.
vi.mock('@/components/ui/dropdown-menu', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    DropdownMenu: plain('dropdown-menu'),
    DropdownMenuTrigger: plain('dropdown-trigger'),
    DropdownMenuContent: plain('dropdown-content'),
    DropdownMenuItem: plain('dropdown-item'),
  }
})

const { default: AppHeader } = await import('./AppHeader.vue')

function account(id: number, username: string, active: boolean, tracked = true, label = username) {
  return { id, username, label, tracked, active }
}

beforeEach(() => {
  state.view = { accounts: [], current: null }
  state.switched = []
  state.pushed = []
  state.path = '/'
  state.context = { current: 'mtgo', mtgo: true, arena: false, endstep: false, switchable: false }
  state.switchedPlatforms = []
  state.info = { productName: 'Qapp', prerelease: true }
})

function mount() {
  const mounted = mountFake(AppHeader)
  expect(mounted.warnings).toEqual([])
  return mounted
}

describe('the header account switcher', () => {
  it('shows nothing without a current account', () => {
    const mounted = mount()
    expect(findAll(mounted.root, (node) => node.tag === 'dropdown-menu')).toHaveLength(0)
    expect(findAll(mounted.root, (node) => node.props['data-account'] !== undefined)).toHaveLength(0)
  })

  it('shows the plain username with one tracked account', () => {
    state.view = {
      accounts: [account(1, 'Qplayer', true), account(2, 'Qhidden', false, false)],
      current: 'Qplayer',
    }
    const mounted = mount()
    expect(findAll(mounted.root, (node) => node.tag === 'dropdown-menu')).toHaveLength(0)
    const name = findAll(mounted.root, (node) => node.props['data-account'] !== undefined)
    expect(name.map((node) => textOf(node).trim())).toEqual(['Qplayer'])
  })

  it('is a dropdown of the tracked accounts with two, and a choice switches and opens the decks', async () => {
    state.view = {
      accounts: [account(1, 'Qplayer', true), account(2, 'Qsecond', false), account(3, 'Qhidden', false, false)],
      current: 'Qplayer',
    }
    const mounted = mount()
    const trigger = findAll(mounted.root, (node) => node.tag === 'dropdown-trigger')
    expect(textOf(trigger[0]!).trim()).toBe('Qplayer')
    const items = findAll(mounted.root, (node) => node.tag === 'dropdown-item')
    expect(items.map((node) => textOf(node).trim())).toEqual(['Qplayer', 'Qsecond'])
    expect(String(items[0]!.props.class)).toContain('font-semibold')
    ;(items[1]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.switched).toEqual(['Qsecond'])
    expect(state.pushed).toEqual(['/decks'])
  })

  it('shows an untagged Arena account by its label and switches by its username', async () => {
    state.view = {
      accounts: [account(1, 'arena:QUSERID1', true, true, 'Qarena'), account(2, 'Qsecond', false)],
      current: 'Qarena',
    }
    const mounted = mount()
    const items = findAll(mounted.root, (node) => node.tag === 'dropdown-item')
    expect(items.map((node) => textOf(node).trim())).toEqual(['Qarena', 'Qsecond'])
    ;(items[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.switched).toEqual(['arena:QUSERID1'])
  })

  it('says No account in the dropdown when none is current', () => {
    state.view = { accounts: [account(1, 'Qplayer', false), account(2, 'Qsecond', false)], current: null }
    const mounted = mount()
    const trigger = findAll(mounted.root, (node) => node.tag === 'dropdown-trigger')
    expect(textOf(trigger[0]!).trim()).toBe('No account')
  })
})

// Release spec 13.1: the tag follows the version, not the product name.
describe('the header Edge tag', () => {
  const tags = (mounted: ReturnType<typeof mount>) =>
    findAll(mounted.root, (node) => node.props['data-edge-tag'] !== undefined)

  it('shows on a prerelease build', () => {
    const shown = tags(mount())
    expect(shown.map((node) => textOf(node).trim())).toEqual(['Edge'])
  })

  it('is gone on a stable build, whatever the name', () => {
    state.info = { productName: 'Qapp Beta', prerelease: false }
    expect(tags(mount())).toHaveLength(0)
  })

  it('waits for the app info', () => {
    state.info = null
    expect(tags(mount())).toHaveLength(0)
  })
})

describe('the top nav (issue #332)', () => {
  /** The labels of the highlighted nav items. */
  function activeItems(): string[] {
    const mounted = mount()
    return findAll(mounted.root, (node) => node.tag === 'a' && hasClass(node, 'shadow-inner')).map((node) =>
      textOf(node).trim(),
    )
  }

  it('keeps Decks active on a match opened inside a deck', () => {
    state.path = '/decks/3/matches/q-token'
    expect(activeItems()).toEqual(['Decks'])
  })

  it('keeps Matches active on a match opened from the Matches page', () => {
    state.path = '/matches/q-token'
    expect(activeItems()).toEqual(['Matches'])
  })
})

describe('the platform context', () => {
  const navLabels = () =>
    findAll(mount().root, (node) => node.tag === 'a' && String(node.props.class).includes('rounded-md border px-3'))
      .map((node) => textOf(node).trim())

  it('the mtgo nav has every page and Import', () => {
    expect(navLabels()).toEqual(['Dashboard', 'Matches', 'Decks', 'Leagues', 'Limited', 'Opponents', 'Metagame', 'Cards'])
    const links = findAll(mount().root, (node) => node.tag === 'a').map((node) => textOf(node).trim())
    expect(links).toContain('Import')
  })

  it('the arena nav has Limited but no Leagues', () => {
    state.context = { current: 'arena', mtgo: true, arena: true, endstep: false, switchable: true }
    expect(navLabels()).toEqual(['Dashboard', 'Matches', 'Decks', 'Limited', 'Opponents', 'Metagame', 'Cards'])
  })

  it('the arena header has no Import', () => {
    state.context = { current: 'arena', mtgo: true, arena: true, endstep: false, switchable: true }
    const links = findAll(mount().root, (node) => node.tag === 'a').map((node) => textOf(node).trim())
    expect(links).not.toContain('Import')
    expect(links).toContain('Settings')
  })

  it('shows the switch when both platforms have data, and a click switches', async () => {
    state.context = { current: 'mtgo', mtgo: true, arena: true, endstep: false, switchable: true }
    const mounted = mount()
    const buttons = findAll(mounted.root, (node) => node.tag === 'button' && node.props['aria-pressed'] !== undefined)
    expect(buttons).toHaveLength(2)
    ;(buttons[1]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.switchedPlatforms).toEqual(['arena'])
  })
})
