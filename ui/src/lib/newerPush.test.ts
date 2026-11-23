import { describe, expect, it } from 'vitest'
import { fetchUnlessPushed, pushCounter } from './newerPush'

describe('fetchUnlessPushed', () => {
  it('returns the fetched value when no push arrived meanwhile', async () => {
    const value = await fetchUnlessPushed(
      () => 0,
      async () => 'fetched',
      () => 'cached',
    )
    expect(value).toBe('fetched')
  })

  it('a status fetched before a push never overwrites it', async () => {
    let pushes = 0
    const value = await fetchUnlessPushed(
      () => pushes,
      async () => {
        pushes += 1
        return 'starting'
      },
      () => 'tracking',
    )
    expect(value).toBe('tracking')
  })

  it('returns the fetched value when a push arrived but nothing is cached', async () => {
    let pushes = 0
    const value = await fetchUnlessPushed(
      () => pushes,
      async () => {
        pushes += 1
        return 'fetched'
      },
      () => undefined,
    )
    expect(value).toBe('fetched')
  })
})

describe('pushCounter', () => {
  it('counts the pushes noted', () => {
    const pushes = pushCounter()
    expect(pushes.count()).toBe(0)
    pushes.note()
    pushes.note()
    expect(pushes.count()).toBe(2)
  })
})
