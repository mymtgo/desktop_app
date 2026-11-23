import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { AppInfo, Settings } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  trayAvailable: true,
  closeToTray: true,
  settings: null as unknown as import('vue').Ref<Settings>,
  progress: null as unknown as import('vue').Ref<{ phase: string } | null>,
  loads: 0,
}))

vi.mock('@/lib/queries', () => ({
  useAppInfo: () => ({
    data: ref<AppInfo>({
      version: '1.0.0',
      dataDir: '/data',
      fatal: null,
      trayAvailable: state.trayAvailable,
      productName: 'MyMTGO',
      prerelease: false,
      os: 'windows',
    }),
  }),
  useSettings: () => ({ data: state.settings }),
  useLegacyImportProgress: () => ({ data: state.progress }),
}))
vi.mock('@/composables/useLaunchAtLogin', () => ({
  useLaunchAtLogin: () => ({
    enabled: ref(false),
    changing: ref(false),
    load: async () => {
      state.loads += 1
    },
    set: async () => {},
  }),
}))
vi.mock('@/composables/useSaveSettings', () => ({
  useSaveSettings: () => ({ saving: ref(false), save: async () => true }),
}))
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: { modelValue: Boolean, disabled: Boolean },
      setup: (props) => () => h('switch', { checked: props.modelValue, disabled: props.disabled }),
    }),
  }
})

const { default: BackgroundCard } = await import('./BackgroundCard.vue')

function card(trayAvailable: boolean, closeToTray = true) {
  state.trayAvailable = trayAvailable
  state.closeToTray = closeToTray
  state.settings = ref({ closeToTray } as Settings)
  state.progress = ref(null)
  state.loads = 0
  const mounted = mountFake(BackgroundCard)
  expect(mounted.warnings).toEqual([])
  return { mounted, text: textOf(mounted.root), switches: findAll(mounted.root, (node) => node.tag === 'switch') }
}

describe('the Background card', () => {
  it('reads launch at login again when a 0.x import finishes, not on progress pushes', async () => {
    const { mounted } = card(true)
    expect(state.loads).toBe(1)
    state.progress.value = { phase: 'writing' }
    await mounted.settle()
    state.progress.value = { phase: 'writing' }
    await mounted.settle()
    expect(state.loads).toBe(1)
    state.progress.value = { phase: 'finished' }
    await mounted.settle()
    expect(state.loads).toBe(2)
  })

  it('has 0.x title, description and launch at login strings', () => {
    const { text } = card(true)
    expect(text).toContain('Background')
    expect(text).toContain('Keep MyMTGO running in the system tray and launch automatically when you sign in.')
    expect(text).toContain('Launch at login')
    expect(text).toContain('Start MyMTGO automatically when your computer signs in.')
    expect(text).toContain('Closing the window')
  })

  it('says closing leaves MyMTGO in the tray and offers the close to tray switch when a tray exists', () => {
    const { text, switches } = card(true)
    expect(text).toContain(
      'Closing the main window leaves MyMTGO running in the system tray. Match ingestion continues in the background, so open it again from the tray icon.',
    )
    expect(text).not.toContain("doesn't support a system tray")
    expect(text).toContain('Close to tray')
    expect(switches).toHaveLength(2)
  })

  it('says closing quits when there is no tray, with no close to tray switch', () => {
    const { text, switches } = card(false)
    expect(text).toContain(
      "Your operating system doesn't support a system tray for this app. Closing the main window will quit MyMTGO and stop match ingestion.",
    )
    expect(text).not.toContain('leaves MyMTGO running')
    expect(text).not.toContain('Close to tray')
    expect(switches).toHaveLength(1)
  })

  it('says closing quits when a tray exists but close to tray is off, keeping the switch', () => {
    const { text, switches } = card(true, false)
    expect(text).toContain('Closing the main window will quit MyMTGO and stop match ingestion.')
    expect(text).not.toContain('leaves MyMTGO running')
    expect(text).not.toContain("doesn't support a system tray")
    expect(text).toContain('Close to tray')
    expect(switches).toHaveLength(2)
  })
})
