import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'
import { useDebouncedSearch } from './useDebouncedSearch'

async function settle(ms: number) {
  await nextTick()
  vi.advanceTimersByTime(ms)
  await nextTick()
  await nextTick()
}

describe('useDebouncedSearch', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  function setup(delayMs?: number, normalise?: (value: string) => string) {
    const url = ref('start')
    const applied: string[] = []
    const scope = effectScope()
    const search = scope.run(() =>
      useDebouncedSearch({
        current: () => url.value,
        apply: (next) => {
          applied.push(next)
          url.value = next
        },
        delayMs,
        normalise,
      }),
    )!
    return { url, applied, search, scope }
  }

  it('starts from the current value and applies the trimmed text after the wait', async () => {
    const { search, applied } = setup()
    expect(search.value).toBe('start')
    search.value = ' bolt '
    await settle(299)
    expect(applied).toEqual([])
    await settle(1)
    expect(applied).toEqual(['bolt'])
  })

  it('applies only the last value typed within the wait', async () => {
    const { search, applied } = setup(250)
    search.value = 'b'
    await settle(100)
    search.value = 'bo'
    await settle(249)
    expect(applied).toEqual([])
    await settle(1)
    expect(applied).toEqual(['bo'])
  })

  it('does not apply a value equal to the current one', async () => {
    const { search, applied } = setup()
    search.value = 'start '
    await settle(300)
    expect(applied).toEqual([])
  })

  it('follows the current value when it changes from outside', async () => {
    const { search, url, applied } = setup()
    url.value = 'back'
    await nextTick()
    expect(search.value).toBe('back')
    await settle(300)
    expect(applied).toEqual([])
  })

  it('keeps typing still waiting to be sent over an outside change', async () => {
    const { search, url, applied } = setup()
    search.value = 'typed'
    await nextTick()
    url.value = 'back'
    await nextTick()
    expect(search.value).toBe('typed')
    await settle(300)
    expect(applied).toEqual(['typed'])
  })

  it('uses the given normalise in place of trimming', async () => {
    const { search, applied } = setup(300, (value) => (value.trim() === '' ? '' : value))
    search.value = ' kept '
    await settle(300)
    expect(applied).toEqual([' kept '])
  })

  it('applies nothing once its scope stops', async () => {
    const { search, applied, scope } = setup()
    search.value = 'late'
    await nextTick()
    scope.stop()
    await settle(300)
    expect(applied).toEqual([])
  })
})
