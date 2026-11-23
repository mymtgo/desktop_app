import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { LegacyImportInfo, StatusView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const app = vi.hoisted(() => ({ os: 'windows' }))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/lib/queries', () => ({
  useSettings: () => ({ data: ref(undefined) }),
  useAppInfo: () => ({ data: ref({ os: app.os }) }),
  useLegacyImport: () => ({
    data: ref<LegacyImportInfo>({ detectedPath: 'C:/q/database.sqlite', chosenPath: null, lastImport: null, running: false }),
  }),
}))
vi.mock('@/composables/useSaveSettings', () => ({
  useSaveSettings: () => ({ saving: ref(false), chooseLogFolder: async () => false }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))

const { default: FirstRunState } = await import('./FirstRunState.vue')

const status = { searched: [], files: [] } as unknown as StatusView

beforeEach(() => {
  app.os = 'windows'
})

describe('the first-run state', () => {
  it('tells a Mac user with no log about virtual machines', () => {
    const searched = { searched: ['/Users/q/.wine'], files: [] } as unknown as StatusView
    const hint = 'Running MTGO in a Windows virtual machine?'
    expect(textOf(mountFake(FirstRunState, { state: 'not_found', status: searched }).root)).not.toContain(hint)
    app.os = 'macos'
    const mounted = mountFake(FirstRunState, { state: 'not_found', status: searched })
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toContain(hint)
  })

  it('points a 0.x user to the Import page', () => {
    const mounted = mountFake(FirstRunState, { state: 'waiting_for_match', status })
    expect(mounted.warnings).toEqual([])
    const link = findAll(mounted.root, (node) => node.tag === 'a').find((node) => textOf(node).includes('Import'))
    expect(link?.props.href).toBe('/import')
    expect(textOf(mounted.root)).toContain('You used MyMTGO 0.x on this computer.')
  })
})
