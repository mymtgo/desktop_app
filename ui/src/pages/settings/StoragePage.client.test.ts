import { describe, expect, it, vi } from 'vitest'
import { mountFake, textOf } from '@/replay/replayTestHarness'

async function stub(name: string) {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ name, setup: () => () => h('card', name) }) }
}

vi.mock('@/components/settings/LocalImagesCard.vue', () => stub('Card images'))
vi.mock('@/components/settings/CardDataCard.vue', () => stub('Card data'))
vi.mock('@/components/settings/FilePathsCard.vue', () => stub('File Paths'))
vi.mock('@/components/settings/WatcherCard.vue', () => stub('Watcher'))

const { default: StoragePage } = await import('./StoragePage.vue')

describe('the Storage settings page', () => {
  // 0.x Pages/StorageControllerTest "renders the storage settings page with path, watcher and image props".
  it('holds the cards in 0.x order, the deleted items moved to Archives', () => {
    const mounted = mountFake(StoragePage)
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toBe('Card imagesCard dataFile PathsWatcher')
  })
})
