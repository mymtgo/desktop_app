import { describe, expect, it, vi } from 'vitest'

const trackUsage = vi.fn()
vi.mock('@/bindings', () => ({ commands: { trackUsage } }))

const { trackUsage: track } = await import('./usage')

describe('trackUsage', () => {
  it('passes the event to the command', () => {
    trackUsage.mockResolvedValue({ status: 'ok', data: null })
    track({ kind: 'pageViewed', page: 'dashboard' })
    expect(trackUsage).toHaveBeenCalledWith({ kind: 'pageViewed', page: 'dashboard' })
  })

  it('never lets a failure reach the page', async () => {
    trackUsage.mockRejectedValue(new Error('no backend'))
    expect(() => track({ kind: 'featureUsed', feature: 'copyImage' })).not.toThrow()
    await Promise.resolve()
  })
})
