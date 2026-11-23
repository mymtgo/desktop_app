import { beforeEach, describe, expect, it, vi } from 'vitest'

const autostart = { enable: vi.fn(), disable: vi.fn(), isEnabled: vi.fn() }
const add = vi.fn()
const launchAtLoginChanged = vi.fn()

vi.mock('@tauri-apps/plugin-autostart', () => autostart)
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))
vi.mock('@/bindings', () => ({ commands: { launchAtLoginChanged } }))

const { useLaunchAtLogin } = await import('./useLaunchAtLogin')

describe('useLaunchAtLogin', () => {
  beforeEach(() => {
    autostart.enable.mockReset()
    autostart.disable.mockReset()
    autostart.isEnabled.mockReset()
    add.mockReset()
    launchAtLoginChanged.mockReset()
    launchAtLoginChanged.mockResolvedValue({ status: 'ok', data: null })
  })

  it('is busy while enable or disable runs, and ignores a second change', async () => {
    let finish = () => {}
    autostart.enable.mockImplementation(() => new Promise<void>((resolve) => (finish = resolve)))
    autostart.isEnabled.mockResolvedValue(true)
    const { enabled, changing, set } = useLaunchAtLogin()

    const first = set(true)
    expect(changing.value).toBe(true)
    await set(false)
    expect(autostart.disable).not.toHaveBeenCalled()

    finish()
    await first
    expect(changing.value).toBe(false)
    expect(enabled.value).toBe(true)
  })

  it('shows a toast and stops being busy when the change fails', async () => {
    autostart.enable.mockRejectedValue(new Error('denied'))
    const { changing, set } = useLaunchAtLogin()

    await set(true)

    expect(changing.value).toBe(false)
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', message: 'denied' }))
  })

  // Release spec section 13 item 4: the 0.x import never replaces a choice made here.
  it('records that the user changed it, so the 0.x import leaves it alone', async () => {
    autostart.disable.mockResolvedValue(undefined)
    autostart.isEnabled.mockResolvedValue(false)
    const { set } = useLaunchAtLogin()

    await set(false)

    expect(launchAtLoginChanged).toHaveBeenCalledTimes(1)
  })

  it('records nothing when the change fails', async () => {
    autostart.enable.mockRejectedValue(new Error('denied'))
    const { set } = useLaunchAtLogin()

    await set(true)

    expect(launchAtLoginChanged).not.toHaveBeenCalled()
  })

  it('shows a toast when the choice cannot be recorded', async () => {
    autostart.disable.mockResolvedValue(undefined)
    autostart.isEnabled.mockResolvedValue(false)
    launchAtLoginChanged.mockResolvedValue({ status: 'error', error: 'db busy' })
    const { set } = useLaunchAtLogin()

    await set(false)

    expect(add).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', message: 'db busy' }))
  })
})
