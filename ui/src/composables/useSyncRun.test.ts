import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useSyncRun } from './useSyncRun'

describe('useSyncRun ("Sync now")', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('clears the busy state at once when the press is refused', async () => {
    const run = useSyncRun({
      start: vi.fn().mockRejectedValue(new Error('Sign in from Settings before syncing decks.')),
      poll: vi.fn(),
      onEnd: vi.fn(),
    })
    await run.press()
    expect(run.running.value).toBe(false)
    expect(run.problem.value).toBe('Sign in from Settings before syncing decks.')
  })

  it('stays busy until a poll says the run ended, then tells the card', async () => {
    const poll = vi.fn().mockResolvedValueOnce({ syncing: true }).mockResolvedValueOnce({ syncing: false })
    const onEnd = vi.fn()
    const run = useSyncRun({ start: vi.fn().mockResolvedValue({ syncing: true }), poll, onEnd })
    await run.press()
    expect(run.running.value).toBe(true)
    await vi.advanceTimersByTimeAsync(3000)
    expect(run.running.value).toBe(true)
    await vi.advanceTimersByTimeAsync(3000)
    expect(run.running.value).toBe(false)
    expect(onEnd).toHaveBeenCalledTimes(1)
  })

  it('stops waiting when the status can no longer be read (tracking restarted)', async () => {
    const run = useSyncRun({
      start: vi.fn().mockResolvedValue({ syncing: true }),
      poll: vi.fn().mockResolvedValue(null),
      onEnd: vi.fn(),
    })
    await run.press()
    await vi.advanceTimersByTimeAsync(3000)
    expect(run.running.value).toBe(false)
  })

  it('gives up after the deadline', async () => {
    const run = useSyncRun({
      start: vi.fn().mockResolvedValue({ syncing: true }),
      poll: vi.fn().mockResolvedValue({ syncing: true }),
      onEnd: vi.fn(),
      deadlineMs: 6000,
    })
    await run.press()
    await vi.advanceTimersByTimeAsync(9000)
    expect(run.running.value).toBe(false)
  })
})
