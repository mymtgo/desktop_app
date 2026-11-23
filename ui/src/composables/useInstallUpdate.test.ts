import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const installUpdate = vi.fn()
const add = vi.fn()
vi.mock('@/bindings', () => ({ commands: { installUpdate } }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))

/** A fresh module each test: `installing` is shared by every caller. */
async function load() {
  vi.resetModules()
  return (await import('./useInstallUpdate')).useInstallUpdate
}

describe('useInstallUpdate', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    installUpdate.mockReset()
    add.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('shows the overlay for 3 s before the install starts, as 0.x', async () => {
    installUpdate.mockReturnValue(new Promise(() => {}))
    const useInstallUpdate = await load()
    const { installing, install } = useInstallUpdate()
    void install()
    expect(installing.value).toBe(true)
    await vi.advanceTimersByTimeAsync(2_999)
    expect(installUpdate).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(1)
    expect(installUpdate).toHaveBeenCalledTimes(1)
    expect(installing.value).toBe(true)
  })

  it('is one install for every surface (banner, card, status bar, overlay)', async () => {
    installUpdate.mockReturnValue(new Promise(() => {}))
    const useInstallUpdate = await load()
    const banner = useInstallUpdate()
    const overlay = useInstallUpdate()
    void banner.install()
    void overlay.install()
    expect(overlay.installing.value).toBe(true)
    await vi.advanceTimersByTimeAsync(3_000)
    expect(installUpdate).toHaveBeenCalledTimes(1)
  })

  it('shows why the install failed and can be tried again', async () => {
    installUpdate.mockResolvedValue({ status: 'error', error: { kind: 'update', message: 'no update is ready to install' } })
    const useInstallUpdate = await load()
    const { installing, install } = useInstallUpdate()
    const done = install()
    await vi.advanceTimersByTimeAsync(3_000)
    await done
    expect(installing.value).toBe(false)
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', message: 'no update is ready to install' }))
  })
})
