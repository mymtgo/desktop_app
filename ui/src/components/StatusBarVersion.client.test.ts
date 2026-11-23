import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { UpdateStatus } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  update: null as UpdateStatus | null,
  installs: 0,
  installing: false,
}))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/lib/queries', () => ({
  useUpdateStatus: () => ({ data: ref(state.update) }),
  useAppInfo: () => ({ data: ref({ version: '1.0.0-beta.2', dataDir: '/d', fatal: null, trayAvailable: true }) }),
}))
vi.mock('@/composables/useInstallUpdate', () => ({
  useInstallUpdate: () => ({
    installing: ref(state.installing),
    install: () => {
      state.installs += 1
    },
  }),
}))

const { default: StatusBarVersion } = await import('./StatusBarVersion.vue')

function status(phase: UpdateStatus['phase'], available: string | null = null): UpdateStatus {
  return { current: '1.0.0-beta.2', available, phase, checkedAtMs: 1, error: null, active: true, channel: 'edge' }
}

function mount(props: Record<string, unknown> = {}) {
  const mounted = mountFake(StatusBarVersion, props)
  expect(mounted.warnings).toEqual([])
  return mounted.root
}

beforeEach(() => {
  state.update = null
  state.installs = 0
  state.installing = false
})

describe('the status bar version item', () => {
  it('offers the ready update and installs it on click', () => {
    state.update = status('ready', '1.0.0')
    const root = mount()
    expect(textOf(root).trim()).toBe('Update ready: v1.0.0')
    const [button] = findAll(root, (node) => node.tag === 'button')
    expect(button?.props.disabled).toBe(false)
    ;(button?.props.onClick as () => void)()
    expect(state.installs).toBe(1)
  })

  it('is disabled while the install starts', () => {
    state.update = status('ready', '1.0.0')
    state.installing = true
    const [button] = findAll(mount(), (node) => node.tag === 'button')
    expect(button?.props.disabled).toBe(true)
  })

  it('says the update is downloading', () => {
    state.update = status('downloading', '1.0.0')
    expect(textOf(mount())).toBe('Downloading update...')
  })

  it('shows the running version otherwise', () => {
    for (const phase of ['up_to_date', 'checking', 'error'] as const) {
      state.update = status(phase)
      const root = mount()
      expect(textOf(root)).toBe('v1.0.0-beta.2')
      expect(findAll(root, (node) => node.tag === 'a')).toEqual([])
    }
  })

  it('shows the app version before the update status arrives', () => {
    expect(textOf(mount())).toBe('v1.0.0-beta.2')
  })

  it("links the version to What's new when the page exists", () => {
    state.update = status('up_to_date')
    const [link] = findAll(mount({ whatsNewUrl: '/whats-new' }), (node) => node.tag === 'a')
    expect(link?.props.href).toBe('/whats-new')
    expect(textOf(link!)).toBe('v1.0.0-beta.2')
  })
})
