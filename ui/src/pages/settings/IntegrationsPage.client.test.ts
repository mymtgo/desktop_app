import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { ConnectionView, IntegrationsView } from '@/bindings'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  view: null as unknown,
  calls: [] as string[],
  cache: [] as unknown[],
  invalidated: [] as unknown[],
  refetches: 0,
  accountChanges: 0,
  answer: null as unknown,
  fail: false,
  openFails: false,
  toasts: [] as unknown[],
}))

vi.mock('@/bindings', () => ({
  commands: {
    setIntegration: async (integration: string, on: boolean) => {
      state.calls.push(`setIntegration:${integration}:${on}`)
      if (state.fail) return { status: 'error', error: 'The settings could not be saved' }
      return { status: 'ok', data: state.answer }
    },
    openExternal: async (url: string) => {
      state.calls.push(`openExternal:${url}`)
      if (state.openFails) return { status: 'error', error: { kind: 'refused', message: 'This link cannot be opened.' } }
      return { status: 'ok', data: null }
    },
  },
}))
vi.mock('@/lib/queries', () => ({
  useIntegrationsQuery: () => ({
    data: ref(state.view),
    refetch: async () => {
      state.refetches += 1
    },
  }),
}))
vi.mock('@/lib/liveEvents', () => ({
  onAccountChanged: () => {
    state.accountChanges += 1
  },
}))
vi.mock('@/queryClient', () => ({
  queryClient: {
    setQueryData: (_key: unknown, data: unknown) => state.cache.push(data),
    invalidateQueries: async (filter: unknown) => {
      state.invalidated.push(filter)
    },
  },
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: (toast: unknown) => state.toasts.push(toast) }) }))
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: ['modelValue', 'disabled'],
      emits: ['update:modelValue'],
      setup: (p, { emit }) => () =>
        h('sw', { on: p.modelValue, disabled: p.disabled, onClick: () => emit('update:modelValue', !p.modelValue) }),
    }),
  }
})
vi.mock('@/components/ui/button', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Button: defineComponent({
      props: ['disabled'],
      setup: (p, { slots }) => () => h('btn', { disabled: p.disabled }, slots.default?.()),
    }),
  }
})
vi.mock('@/components/ui/label', async () => {
  const { defineComponent, h } = await import('vue')
  return { Label: defineComponent({ setup: (_p, { slots }) => () => h('label', {}, slots.default?.()) }) }
})

const { default: IntegrationsPage } = await import('./IntegrationsPage.vue')

function viewWith(connection: ConnectionView, fields: Partial<IntegrationsView> = {}): IntegrationsView {
  return { arena: true, endstep: true, endstepStatus: { helperMissing: false, connection, hasAccount: false }, ...fields }
}

async function mountPage(view: IntegrationsView, props: Record<string, unknown> = {}) {
  state.view = view
  state.answer = view
  const mounted = mountFake(IntegrationsPage, props)
  await mounted.settle()
  return mounted
}

const switches = (root: FakeNode) => findAll(root, (n) => n.tag === 'sw')

beforeEach(() => {
  state.calls = []
  state.cache = []
  state.invalidated = []
  state.refetches = 0
  state.fail = false
  state.openFails = false
  state.accountChanges = 0
  state.toasts = []
})

describe('the Integrations settings page', () => {
  it('renders a switch for Arena and Endstep and not for MTGO', async () => {
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }, { endstep: false }))
    const sws = switches(mounted.root)
    expect(sws.map((s) => s.props.on)).toEqual([true, false])
    const text = textOf(mounted.root)
    expect(text).toContain("Reading MTG Arena's Player.log")
    expect(text).toContain('Off. Captures wait in the browser extension')
    expect(findAll(mounted.root, (n) => n.tag === 'label').map((n) => textOf(n).trim())).toEqual(['MTG Arena', 'Endstep'])
    mounted.unmount()
  })

  it('turning Endstep on calls setIntegration, writes the cache and refreshes the platform context', async () => {
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }, { endstep: false }))
    ;(switches(mounted.root)[1]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['setIntegration:endstep:true'])
    expect(state.cache).toHaveLength(1)
    expect(state.invalidated).toEqual([{ queryKey: ['platformContext'] }])
    mounted.unmount()
  })

  it('turning Arena off calls setIntegration for arena', async () => {
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }))
    ;(switches(mounted.root)[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['setIntegration:arena:false'])
    mounted.unmount()
  })

  it('never seen shows the install steps and Check again', async () => {
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }), { storeLinks: { chrome: '', firefox: '' } })
    const text = textOf(mounted.root)
    expect(text).toContain('Not connected yet')
    expect(text).toContain('Chrome, Edge and Brave')
    expect(text).toContain('Load unpacked')
    expect(text).toContain('about:debugging')
    expect(text).toContain('Load Temporary Add-on')
    expect(text).toContain('Flatpak browsers cannot talk to MyMTGO.')
    expect(text.toLowerCase()).not.toContain('not installed')
    const again = findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Check again')
    expect(again).toHaveLength(1)
    ;(again[0]!.props.onClick as () => void)()
    expect(state.refetches).toBe(1)
    mounted.unmount()
  })

  it('recent shows the browser and version without install steps', async () => {
    const mounted = await mountPage(viewWith({ kind: 'recent', browser: 'firefox', extension: '0.4.1', atMs: Date.now() }))
    const text = textOf(mounted.root)
    expect(text).toContain('Connected: Firefox, extension 0.4.1, last seen just now')
    expect(text).not.toContain('Load unpacked')
    expect(findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Check again')).toHaveLength(0)
    mounted.unmount()
  })

  it('a failed toggle toasts once, writes nothing and leaves the switches usable', async () => {
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }, { endstep: false }))
    state.fail = true
    ;(switches(mounted.root)[1]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.toasts).toHaveLength(1)
    expect(state.cache).toEqual([])
    expect(state.invalidated).toEqual([])
    expect(switches(mounted.root).map((s) => s.props.disabled)).toEqual([false, false])
    mounted.unmount()
  })

  it('keeps the install steps once connected, with no Check again', async () => {
    const stale: ConnectionView = { kind: 'stale', browser: 'chrome', extension: '0.4.1', atMs: 0 }
    const recent: ConnectionView = { kind: 'recent', browser: 'chrome', extension: '0.4.1', atMs: Date.now() }
    for (const connection of [stale, recent]) {
      const mounted = await mountPage(viewWith(connection), { storeLinks: { chrome: '', firefox: '' } })
      expect(textOf(mounted.root)).toContain('Load unpacked')
      expect(findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Check again')).toHaveLength(0)
      mounted.unmount()
    }
  })

  it('says matches appear after the first one only while no Endstep account exists', async () => {
    const line = 'Matches appear here after your first Endstep match.'
    const without = await mountPage(viewWith({ kind: 'neverSeen' }))
    expect(textOf(without.root)).toContain(line)
    without.unmount()
    const withAccount = viewWith({ kind: 'neverSeen' })
    withAccount.endstepStatus.hasAccount = true
    const mounted = await mountPage(withAccount)
    expect(textOf(mounted.root)).not.toContain(line)
    mounted.unmount()
  })

  it('switching a platform off refetches the pages like an account change', async () => {
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }))
    ;(switches(mounted.root)[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.accountChanges).toBe(1)
    ;(switches(mounted.root)[1]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.accountChanges).toBe(2)
    mounted.unmount()
  })

  it('hides the install steps when the helper is missing and when Endstep is off', async () => {
    const cases: IntegrationsView[] = [
      { ...viewWith({ kind: 'neverSeen' }), endstepStatus: { helperMissing: true, connection: { kind: 'neverSeen' }, hasAccount: false } },
      viewWith({ kind: 'neverSeen' }, { endstep: false }),
    ]
    for (const view of cases) {
      const mounted = await mountPage(view)
      const text = textOf(mounted.root)
      expect(text).not.toContain('Load unpacked')
      expect(text).not.toContain('Flatpak')
      expect(findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Check again')).toHaveLength(0)
      mounted.unmount()
    }
  })

  it('a_store_link_replaces_the_manual_steps', async () => {
    const links = { chrome: 'https://chromewebstore.google.com/detail/x', firefox: 'https://addons.mozilla.org/a/x' }
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }), { storeLinks: links })
    const text = textOf(mounted.root)
    expect(text).toContain('Add to Chrome')
    expect(text).toContain('Add to Firefox')
    expect(text).toContain('Edge and Brave')
    expect(text).not.toContain('Load unpacked')
    expect(text).not.toContain('about:debugging')
    expect(text).toContain('Flatpak browsers cannot talk to MyMTGO.')
    expect(findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Check again')).toHaveLength(1)
    mounted.unmount()
  })

  it('an_empty_or_foreign_link_keeps_the_manual_steps', async () => {
    const links = { chrome: '', firefox: 'https://example.com/addon' }
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }), { storeLinks: links })
    const text = textOf(mounted.root)
    expect(text).toContain('Load unpacked')
    expect(text).toContain('Load Temporary Add-on')
    expect(text).not.toContain('Add to Chrome')
    expect(text).not.toContain('Add to Firefox')
    mounted.unmount()
  })

  it('the_button_opens_the_listing', async () => {
    const url = 'https://chromewebstore.google.com/detail/x'
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }), { storeLinks: { chrome: url, firefox: '' } })
    const button = findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Add to Chrome')
    expect(button).toHaveLength(1)
    ;(button[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual([`openExternal:${url}`])
    mounted.unmount()
  })

  it('a_refused_open_shows_the_error', async () => {
    const url = 'https://chromewebstore.google.com/detail/x'
    state.openFails = true
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }), { storeLinks: { chrome: url, firefox: '' } })
    const button = findAll(mounted.root, (n) => n.tag === 'btn' && textOf(n).trim() === 'Add to Chrome')
    ;(button[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.toasts).toHaveLength(1)
    expect(JSON.stringify(state.toasts[0])).toContain('This link cannot be opened.')
    mounted.unmount()
  })

  it('shows no store buttons while the install block is hidden', async () => {
    const links = { chrome: 'https://chromewebstore.google.com/detail/x', firefox: '' }
    const mounted = await mountPage(viewWith({ kind: 'neverSeen' }, { endstep: false }), { storeLinks: links })
    expect(textOf(mounted.root)).not.toContain('Add to Chrome')
    mounted.unmount()
  })
})
