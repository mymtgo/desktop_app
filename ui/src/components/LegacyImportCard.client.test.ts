import { afterEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { ImportSummary, LegacyImportInfo } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ lastImport: null as ImportSummary | null }))

vi.mock('@/bindings', () => ({ commands: {} }))
vi.mock('@/queryClient', () => ({ queryClient: {} }))
vi.mock('@/lib/queries', () => ({
  useLegacyImport: () => ({
    data: ref<LegacyImportInfo>({
      detectedPath: 'C:/Users/Q/AppData/Roaming/mymtgo/database/database.sqlite',
      chosenPath: null,
      lastImport: state.lastImport,
      running: false,
    }),
  }),
  useLegacyImportProgress: () => ({ data: ref(null) }),
}))

const { default: LegacyImportCard } = await import('./LegacyImportCard.vue')

describe('the 0.x import section', () => {
  afterEach(() => {
    state.lastImport = null
  })

  it('is headed MyMTGO 0.x', () => {
    const mounted = mountFake(LegacyImportCard)
    expect(mounted.warnings).toEqual([])
    const headings = findAll(mounted.root, (node) => node.tag === 'h2').map((node) => textOf(node).trim())
    expect(headings[0]).toBe('MyMTGO 0.x')
    expect(textOf(mounted.root)).toContain('Import from MyMTGO 0.x')
  })

  // Release spec section 13 item 3.
  it('says 0.x can be uninstalled once an import finished', () => {
    const uninstall = 'You can uninstall MyMTGO 0.x now. Its data stays on this computer, so you can import again.'
    expect(textOf(mountFake(LegacyImportCard).root)).not.toContain(uninstall)

    state.lastImport = { finishedAtMs: Date.now(), path: 'Q.sqlite', added: 3, updated: 0, unchanged: 0 } as ImportSummary
    const text = textOf(mountFake(LegacyImportCard).root)
    expect(text).toContain(uninstall)
    expect(text).not.toContain('Until then')
  })

  // #307 F1: pre-release per-game replay links stay on mymtgo.com.
  it('says where to switch off the replay links it left behind', () => {
    state.lastImport = {
      finishedAtMs: Date.now(),
      path: 'Q.sqlite',
      added: 3,
      updated: 0,
      unchanged: 0,
      replaySharesAdded: 2,
      legacyGameLinks: 1,
    } as ImportSummary
    const text = textOf(mountFake(LegacyImportCard).root)
    expect(text).toContain('Replays: 2 shared replay links brought over.')
    expect(text).toContain('switch it off on your Account page on mymtgo.com.')
    state.lastImport = null
  })
})
