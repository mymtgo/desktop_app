import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { Settings, UpdateStatus } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  update: null as UpdateStatus | null,
  settings: { betaUpdates: null } as unknown as Settings,
  chosen: [] as boolean[],
  stored: [] as unknown[],
  productName: 'MyMTGO' as string | null,
}))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/lib/queries', () => ({
  useUpdateStatus: () => ({ data: ref(state.update) }),
  useSettings: () => ({ data: ref(state.settings) }),
  useAppInfo: () => ({ data: ref(state.productName === null ? undefined : { productName: state.productName }) }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))
vi.mock('@/composables/useInstallUpdate', () => ({
  useInstallUpdate: () => ({ installing: ref(false), install: () => {} }),
}))
vi.mock('@/bindings', () => ({
  commands: {
    setBetaUpdates: async (on: boolean) => {
      state.chosen.push(on)
      return { status: 'ok', data: { betaUpdates: on } }
    },
  },
}))
vi.mock('@/queryClient', () => ({
  queryClient: {
    setQueryData: (key: unknown, value: unknown) => {
      state.stored.push([key, value])
    },
  },
}))
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: { modelValue: Boolean, disabled: Boolean },
      emits: ['update:modelValue'],
      setup: (props, { emit }) => () =>
        h('switch', {
          checked: props.modelValue,
          disabled: props.disabled,
          onToggle: (on: boolean) => emit('update:modelValue', on),
        }),
    }),
  }
})

const { default: UpdatesCard } = await import('./UpdatesCard.vue')

function status(channel: UpdateStatus['channel'], current = '1.0.0-beta.2'): UpdateStatus {
  return { current, available: null, phase: 'up_to_date', checkedAtMs: null, error: null, active: true, channel }
}

function mount(props: Record<string, unknown> = {}) {
  const mounted = mountFake(UpdatesCard, props)
  expect(mounted.warnings).toEqual([])
  const [toggle] = findAll(mounted.root, (node) => node.tag === 'switch')
  return { root: mounted.root, text: textOf(mounted.root), toggle: toggle! }
}

beforeEach(() => {
  state.update = status('edge')
  state.settings = { betaUpdates: null } as unknown as Settings
  state.chosen = []
  state.stored = []
  state.productName = 'MyMTGO'
})

describe('the Updates card', () => {
  it('names the version and its channel', () => {
    expect(mount().text).toContain('MyMTGO v1.0.0-beta.2 (edge)')
    state.update = status('stable', '1.0.0')
    expect(mount().text).toContain('MyMTGO v1.0.0 (stable)')
  })

  it('reads the product name at runtime (release spec 13.1)', () => {
    state.productName = 'Qapp'
    expect(mount().text).toContain('Qapp v1.0.0-beta.2 (edge)')
    // Before the app info arrives, the brand.
    state.productName = null
    expect(mount().text).toContain('MyMTGO v1.0.0-beta.2 (edge)')
  })

  it('has the edge switch with its description, on for the edge channel', () => {
    const { text, toggle } = mount()
    expect(text).toContain('Enable edge updates')
    expect(text).toContain('Get new versions before everyone else. Edge builds can have rough edges.')
    expect(toggle.props.checked).toBe(true)
    state.update = status('stable', '1.0.0')
    expect(mount().toggle.props.checked).toBe(false)
  })

  it('saves the choice through its own command, never a whole settings save', async () => {
    const { toggle } = mount()
    ;(toggle.props.onToggle as (on: boolean) => void)(false)
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(state.chosen).toEqual([false])
    expect(state.stored).toContainEqual([['settings'], { betaUpdates: false }])
  })

  it("links What's new when the page exists", () => {
    expect(findAll(mount().root, (node) => node.tag === 'a')).toEqual([])
    const [link] = findAll(mount({ whatsNewUrl: '/whats-new' }).root, (node) => node.tag === 'a')
    expect(link?.props.href).toBe('/whats-new')
    expect(textOf(link!)).toBe("What's new")
  })
})
