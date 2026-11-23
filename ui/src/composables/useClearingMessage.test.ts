import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CLEARING_MESSAGE_MS, useClearingMessage } from './useClearingMessage'

describe('useClearingMessage', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('shows a message, then clears it by itself', () => {
    const { message, show } = useClearingMessage()
    show('The replay could not be opened.')
    expect(message.value).toBe('The replay could not be opened.')
    vi.advanceTimersByTime(CLEARING_MESSAGE_MS - 1)
    expect(message.value).toBe('The replay could not be opened.')
    vi.advanceTimersByTime(1)
    expect(message.value).toBeNull()
  })

  it('restarts the wait when a newer message replaces it', () => {
    const { message, show } = useClearingMessage()
    show('first')
    vi.advanceTimersByTime(CLEARING_MESSAGE_MS - 10)
    show('second')
    vi.advanceTimersByTime(CLEARING_MESSAGE_MS - 10)
    expect(message.value).toBe('second')
    vi.advanceTimersByTime(10)
    expect(message.value).toBeNull()
  })

  it('clears at once on request and stops the timer', () => {
    const { message, show, clear } = useClearingMessage()
    show('x')
    clear()
    expect(message.value).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })
})
