import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { OfflineModeState, StatusView } from '@/bindings'

const status = ref<Partial<StatusView> | undefined>(undefined)
const commands = { offlineMode: vi.fn(), setOfflineMode: vi.fn() }

vi.mock('@/lib/queries', () => ({ useStatus: () => ({ data: status }) }))
vi.mock('@/bindings', () => ({ commands }))

const { useOfflineMode, useOfflineModeSwitch } = await import('./useOfflineMode')

const ok = (data: OfflineModeState) => ({ status: 'ok', data })

describe('useOfflineMode (the status bar pill)', () => {
  it('is on only when the status says offline mode is on', () => {
    const offline = useOfflineMode()
    status.value = undefined
    expect(offline.value).toBe(false)
    status.value = { offlineMode: false }
    expect(offline.value).toBe(false)
    status.value = { offlineMode: true }
    expect(offline.value).toBe(true)
  })
})

describe('useOfflineModeSwitch (0.x OfflineModeCard)', () => {
  beforeEach(() => {
    commands.offlineMode.mockReset()
    commands.setOfflineMode.mockReset()
  })

  it('turns offline mode on straight away, without a dialog', async () => {
    commands.setOfflineMode.mockResolvedValue(ok({ enabled: true, lockedUntilMs: null }))
    const offline = useOfflineModeSwitch()
    await offline.toggle(true)
    expect(offline.dialogOpen.value).toBe(false)
    expect(commands.setOfflineMode).toHaveBeenCalledWith(true)
    expect(offline.state.value).toEqual({ enabled: true, lockedUntilMs: null })
  })

  it('asks before coming back online, and stays offline on "Stay offline"', async () => {
    const offline = useOfflineModeSwitch()
    await offline.toggle(false)
    expect(offline.dialogOpen.value).toBe(true)
    expect(commands.setOfflineMode).not.toHaveBeenCalled()
    offline.dialogOpen.value = false
    expect(commands.setOfflineMode).not.toHaveBeenCalled()
  })

  it('comes back online once confirmed', async () => {
    commands.setOfflineMode.mockResolvedValue(ok({ enabled: false, lockedUntilMs: 1_800_000_000_000 }))
    const offline = useOfflineModeSwitch()
    await offline.toggle(false)
    await offline.confirmRejoin()
    expect(offline.dialogOpen.value).toBe(false)
    expect(commands.setOfflineMode).toHaveBeenCalledWith(false)
    expect(offline.state.value).toEqual({ enabled: false, lockedUntilMs: 1_800_000_000_000 })
    expect(offline.switching.value).toBe(false)
  })

  it('shows a refusal and reloads what is stored', async () => {
    const message =
      'Offline mode is on cooldown after coming back online. You can turn it back on 5 hours from now.'
    commands.setOfflineMode.mockResolvedValue({ status: 'error', error: { kind: 'refused', message } })
    commands.offlineMode.mockResolvedValue(ok({ enabled: false, lockedUntilMs: 1 }))
    const offline = useOfflineModeSwitch()
    await offline.toggle(true)
    expect(offline.problem.value).toBe(message)
    expect(offline.state.value).toEqual({ enabled: false, lockedUntilMs: 1 })
    expect(offline.switching.value).toBe(false)
  })

  it('tells the card after a switch, so it can reload what the switch changed', async () => {
    const changed = vi.fn()
    commands.setOfflineMode.mockResolvedValue(ok({ enabled: false, lockedUntilMs: 1 }))
    const offline = useOfflineModeSwitch(changed)
    await offline.toggle(false)
    await offline.confirmRejoin()
    expect(changed).toHaveBeenCalledTimes(1)
  })

  it('tells the card after a refused switch too', async () => {
    const changed = vi.fn()
    commands.setOfflineMode.mockResolvedValue({ status: 'error', error: { kind: 'refused', message: 'no' } })
    commands.offlineMode.mockResolvedValue(ok({ enabled: false, lockedUntilMs: 1 }))
    const offline = useOfflineModeSwitch(changed)
    await offline.toggle(true)
    expect(changed).toHaveBeenCalledTimes(1)
  })
})
