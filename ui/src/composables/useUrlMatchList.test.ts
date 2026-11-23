import { beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, reactive, ref } from 'vue'
import type { ShownMatchPage } from './useUrlMatchList'

const router = vi.hoisted(() => ({
  route: null as unknown as { query: Record<string, string> },
  push: vi.fn(),
  replace: vi.fn(),
}))

vi.mock('vue-router', () => ({
  useRoute: () => router.route,
  useRouter: () => ({ push: router.push, replace: router.replace }),
}))

vi.mock('@/composables/usePlatformContext', () => ({ usePlatformContext: () => ({ platform: ref('mtgo') }) }))

const { emptiedPageTarget, useUrlMatchList } = await import('./useUrlMatchList')

function page(page: number, rows: number, total: number): ShownMatchPage {
  return { page, rows: Array.from({ length: rows }, () => ({})), total, perPage: 50 }
}

describe('emptiedPageTarget', () => {
  it('steps back from an emptied page', () => {
    expect(emptiedPageTarget(page(3, 0, 100), 3)).toBe(2)
  })

  it('goes to the last page when far beyond it', () => {
    expect(emptiedPageTarget(page(9, 0, 60), 9)).toBe(2)
  })

  it('leaves a page with rows, the first page, a missing page and a stale page alone', () => {
    expect(emptiedPageTarget(page(2, 1, 51), 2)).toBeNull()
    expect(emptiedPageTarget(page(1, 0, 0), 1)).toBeNull()
    expect(emptiedPageTarget(undefined, 2)).toBeNull()
    expect(emptiedPageTarget(page(3, 0, 100), 4)).toBeNull()
  })
})

describe('useUrlMatchList', () => {
  beforeEach(() => {
    router.route = reactive({ query: { page: '3', version: '7' } })
    router.push.mockReset()
    router.replace.mockReset()
  })

  it('reads the list from the URL', () => {
    const { list } = effectScope().run(() => useUrlMatchList())!
    expect(list.value.page).toBe(3)
  })

  it('steps back from an emptied page, replacing the URL and keeping its other keys', async () => {
    const shown = ref<ShownMatchPage | null>(null)
    effectScope().run(() => useUrlMatchList().stepBackFromEmptyPage(shown))
    shown.value = page(3, 0, 100)
    await nextTick()
    expect(router.push).not.toHaveBeenCalled()
    expect(router.replace).toHaveBeenCalledWith({ query: { version: '7', page: '2' } })
  })

  it('writes only the list when asked not to keep other keys', async () => {
    const shown = ref<ShownMatchPage | null>(null)
    effectScope().run(() => useUrlMatchList({ keepOtherQuery: false }).stepBackFromEmptyPage(shown))
    shown.value = page(3, 0, 100)
    await nextTick()
    expect(router.replace).toHaveBeenCalledWith({ query: { page: '2' } })
  })

  it('does not step back while the page has rows', async () => {
    const shown = ref<ShownMatchPage | null>(null)
    effectScope().run(() => useUrlMatchList().stepBackFromEmptyPage(shown))
    shown.value = page(3, 4, 104)
    await nextTick()
    expect(router.replace).not.toHaveBeenCalled()
  })

  it('pushes a new sort on the first page', () => {
    const { sort } = effectScope().run(() => useUrlMatchList())!
    sort('duration')
    expect(router.push).toHaveBeenCalledTimes(1)
    expect(router.push.mock.calls[0][0].query.page).toBeUndefined()
    expect(router.push.mock.calls[0][0].query.sort).toBe('duration')
  })
})
