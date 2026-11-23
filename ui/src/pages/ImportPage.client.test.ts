import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { findAll, mountFake } from '@/replay/replayTestHarness'

vi.mock('@/bindings', () => ({
  commands: { historyScanStatus: () => new Promise(() => {}) },
  events: {
    historyScanChanged: { listen: async () => () => {} },
    historyImportsChanged: { listen: async () => () => {} },
  },
}))
vi.mock('@/lib/queries', () => ({
  keys: {},
  useHistoryDeckVersions: () => ({ data: ref([]) }),
  useHistoryImportedCount: () => ({ data: ref(0) }),
}))
vi.mock('@/queryClient', () => ({ queryClient: {} }))
vi.mock('@/components/LegacyImportCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('legacy-import-card') }) }
})
vi.mock('@/components/import/ScanCardsDialog.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('scan-cards-dialog') }) }
})
vi.mock('@/components/ui/dialog', async () => {
  const { defineComponent } = await import('vue')
  const empty = defineComponent({ setup: () => () => null })
  return {
    Dialog: empty,
    DialogContent: empty,
    DialogDescription: empty,
    DialogFooter: empty,
    DialogHeader: empty,
    DialogTitle: empty,
  }
})

const { default: ImportPage } = await import('./ImportPage.vue')

describe('the Import page', () => {
  it('shows the MyMTGO 0.x import section', () => {
    const mounted = mountFake(ImportPage)
    expect(mounted.warnings).toEqual([])
    expect(findAll(mounted.root, (node) => node.tag === 'legacy-import-card')).toHaveLength(1)
  })
})
