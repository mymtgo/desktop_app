import { describe, expect, it, vi } from 'vitest'
import { createErrorReporter, describeError, MAX_REPORTS, REPEAT_AFTER_MS } from './errors'

describe('createErrorReporter', () => {
  it('sends an error with its stack and route', () => {
    const send = vi.fn().mockResolvedValue(null)
    const report = createErrorReporter(send, () => 0)
    const error = new Error('boom')

    expect(report('window', error, '/settings')).toBe(true)
    expect(send).toHaveBeenCalledWith({ source: 'window', message: 'boom', stack: error.stack ?? null, route: '/settings' })
  })

  it('skips the browser ResizeObserver loop notices, which are not errors', () => {
    const send = vi.fn().mockResolvedValue(null)
    const report = createErrorReporter(send, () => 0)

    expect(report('window', 'ResizeObserver loop completed with undelivered notifications.')).toBe(false)
    expect(report('window', 'ResizeObserver loop limit exceeded')).toBe(false)
    expect(send).not.toHaveBeenCalled()
  })

  it('sends the same message at most once a minute', () => {
    let now = 0
    const send = vi.fn().mockResolvedValue(null)
    const report = createErrorReporter(send, () => now)

    report('window', new Error('again'))
    now = REPEAT_AFTER_MS - 1
    expect(report('window', new Error('again'))).toBe(false)
    now = REPEAT_AFTER_MS
    expect(report('window', new Error('again'))).toBe(true)
    expect(send).toHaveBeenCalledTimes(2)
  })

  it('stops after the session limit', () => {
    const send = vi.fn().mockResolvedValue(null)
    const report = createErrorReporter(send, () => 0)

    for (let index = 0; index < MAX_REPORTS; index += 1) {
      expect(report('window', new Error(`error ${index}`))).toBe(true)
    }
    expect(report('window', new Error('one too many'))).toBe(false)
    expect(send).toHaveBeenCalledTimes(MAX_REPORTS)
  })

  it('never fails when the send itself throws', () => {
    const send = vi.fn().mockImplementation(() => {
      throw new Error('ipc not ready')
    })
    const report = createErrorReporter(send, () => 0)

    expect(() => report('window', 'broken')).not.toThrow()
  })

  it('never fails when sending fails', async () => {
    const send = vi.fn().mockRejectedValue(new Error('ipc gone'))
    const report = createErrorReporter(send, () => 0)

    expect(report('promise', 'rejected')).toBe(true)
    await Promise.resolve()
  })
})

describe('describeError', () => {
  it('describes errors, strings and other values', () => {
    expect(describeError('plain text')).toEqual({ message: 'plain text', stack: null })
    expect(describeError({ code: 7 })).toEqual({ message: '{"code":7}', stack: null })
    expect(describeError(new TypeError(''))).toEqual(expect.objectContaining({ message: 'TypeError' }))
  })
})
