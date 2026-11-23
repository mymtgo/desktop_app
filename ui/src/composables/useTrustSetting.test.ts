import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const commands = { setCardStatsTrust: vi.fn() }
const add = vi.fn()

vi.mock('@/bindings', () => ({ commands }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))

const { useTrustSetting } = await import('./useTrustSetting')

describe('useTrustSetting', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    commands.setCardStatsTrust.mockReset()
    add.mockReset()
    commands.setCardStatsTrust.mockImplementation(async (value: number) => ({ status: 'ok', data: value }))
  })
  afterEach(() => vi.useRealTimers())

  it('saves 300 ms after the last change, once', async () => {
    const trust = useTrustSetting()
    trust.seed(50)
    trust.setValue(60)
    await vi.advanceTimersByTimeAsync(200)
    trust.setValue(70)
    await vi.advanceTimersByTimeAsync(299)
    expect(commands.setCardStatsTrust).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(commands.setCardStatsTrust).toHaveBeenCalledTimes(1)
    expect(commands.setCardStatsTrust).toHaveBeenCalledWith(70)
    expect(trust.value.value).toBe(70)
  })

  it('clamps to 0 through 100,000 and rounds', async () => {
    const trust = useTrustSetting()
    trust.seed(50)
    trust.setValue(250000)
    expect(trust.value.value).toBe(100000)
    trust.setValue(-4)
    expect(trust.value.value).toBe(0)
    trust.setValue(12.6)
    expect(trust.value.value).toBe(13)
  })

  it('reverts to the last saved value and toasts when the save fails', async () => {
    const trust = useTrustSetting()
    trust.seed(50)
    commands.setCardStatsTrust.mockResolvedValue({ status: 'error', error: { kind: 'storage', message: 'x' } })
    trust.setValue(90)
    await vi.advanceTimersByTimeAsync(300)
    expect(trust.value.value).toBe(50)
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not save trust preference',
      message: 'Your slider value will reset next time you open the page.',
    })
  })

  it('resets to 50', async () => {
    const trust = useTrustSetting()
    trust.seed(120)
    trust.reset()
    expect(trust.value.value).toBe(50)
  })

  it('seeds once: a later seed does not overwrite the slider', () => {
    const trust = useTrustSetting()
    trust.seed(120)
    trust.setValue(130)
    trust.seed(10)
    expect(trust.value.value).toBe(130)
  })
})
