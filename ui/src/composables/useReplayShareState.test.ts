import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { ReplayShareState } from '@/bindings'

const commands = vi.hoisted(() => ({
  replayShareState: vi.fn(),
  shareReplay: vi.fn(),
  revokeReplay: vi.fn(),
}))
vi.mock('@/bindings', () => ({ commands }))

const { useReplayShareState } = await import('./useReplayShareState')

const unshared: ReplayShareState = { linked: true, supporter: true, shareable: true, url: null }
const shared: ReplayShareState = { linked: true, supporter: true, shareable: true, url: 'https://mymtgo.test/players/Qplayer/replays/u?game=2' }

const writeText = vi.fn()

beforeEach(() => {
  commands.replayShareState.mockReset()
  commands.shareReplay.mockReset()
  commands.revokeReplay.mockReset()
  writeText.mockReset()
  writeText.mockResolvedValue(undefined)
  vi.stubGlobal('navigator', { clipboard: { writeText } })
})

describe('useReplayShareState', () => {
  it('loads the state of the game on screen', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: unshared })
    const share = useReplayShareState(() => 7)
    await share.refresh()
    expect(commands.replayShareState).toHaveBeenCalledWith(7)
    expect(share.state.value).toEqual(unshared)
  })

  it('is busy while uploading and keeps the new link', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: unshared })
    let finish: (value: unknown) => void = () => {}
    commands.shareReplay.mockReturnValue(new Promise((resolve) => (finish = resolve)))
    const share = useReplayShareState(() => 7)
    const done = share.share()
    expect(share.busy.value).toBe(true)
    finish({ status: 'ok', data: shared })
    await done
    expect(share.busy.value).toBe(false)
    expect(share.state.value).toEqual(shared)
    expect(share.error.value).toBeNull()
  })

  it('shows the message of a refused share and keeps the state', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: unshared })
    commands.shareReplay.mockResolvedValue({ status: 'error', error: 'This game is too large to share.' })
    const share = useReplayShareState(() => 7)
    await share.refresh()
    await share.share()
    expect(share.error.value).toBe('This game is too large to share.')
    expect(share.state.value).toEqual(unshared)
    expect(share.busy.value).toBe(false)
  })

  it('switches the link off and shows a refusal', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: shared })
    commands.revokeReplay.mockResolvedValueOnce({ status: 'error', error: 'Could not reach mymtgo.com. The link is still on; try again in a moment.' })
    commands.revokeReplay.mockResolvedValueOnce({ status: 'ok', data: unshared })
    const share = useReplayShareState(() => 7)
    await share.refresh()
    await share.revoke()
    expect(share.error.value).toContain('The link is still on')
    expect(share.state.value).toEqual(shared)
    await share.revoke()
    expect(share.error.value).toBeNull()
    expect(share.state.value).toEqual(unshared)
  })

  it('copies the link, says Copied, and resets when reopened', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: shared })
    const share = useReplayShareState(() => 7)
    await share.refresh()
    await share.copy()
    expect(writeText).toHaveBeenCalledWith(shared.url)
    expect(share.copied.value).toBe(true)
    await share.reopened()
    expect(share.copied.value).toBe(false)
    expect(commands.replayShareState).toHaveBeenCalledTimes(2)
  })

  it('forgets the error and the copy when the game changes', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: shared })
    commands.shareReplay.mockResolvedValue({ status: 'error', error: 'x' })
    const game = ref(1)
    const share = useReplayShareState(() => game.value)
    await share.refresh()
    await share.share()
    game.value = 2
    await share.refresh()
    expect(share.error.value).toBeNull()
    expect(commands.replayShareState).toHaveBeenLastCalledWith(2)
  })

  it('keeps the new link when a read started during the upload ends after it', async () => {
    let finishRead: (value: unknown) => void = () => {}
    commands.replayShareState.mockReturnValue(new Promise((resolve) => (finishRead = resolve)))
    commands.shareReplay.mockResolvedValue({ status: 'ok', data: shared })
    const share = useReplayShareState(() => 7)
    const read = share.refresh()
    await share.share()
    finishRead({ status: 'ok', data: unshared })
    await read
    expect(share.state.value).toEqual(shared)
  })

  it('drops a share result when the game changed during the upload', async () => {
    const other: ReplayShareState = { linked: true, supporter: true, shareable: true, url: 'https://mymtgo.test/players/Qplayer/replays/u?game=1' }
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: other })
    let finish: (value: unknown) => void = () => {}
    commands.shareReplay.mockReturnValue(new Promise((resolve) => (finish = resolve)))
    const game = ref(2)
    const share = useReplayShareState(() => game.value)
    const done = share.share()
    game.value = 1
    finish({ status: 'ok', data: shared })
    await done
    await Promise.resolve()
    expect(share.state.value).toEqual(other)
    expect(commands.replayShareState).toHaveBeenLastCalledWith(1)
    expect(share.busy.value).toBe(false)
  })

  it('shows a failed read as the error line', async () => {
    commands.replayShareState.mockRejectedValue(new Error('the app is not answering'))
    const share = useReplayShareState(() => 7)
    await expect(share.refresh()).resolves.toBeUndefined()
    expect(share.error.value).toBe('the app is not answering')
    expect(share.state.value).toBeNull()
  })

  it('says so when the link could not be copied', async () => {
    commands.replayShareState.mockResolvedValue({ status: 'ok', data: shared })
    writeText.mockRejectedValue(new Error('denied'))
    const share = useReplayShareState(() => 7)
    await share.refresh()
    await share.copy()
    expect(share.copied.value).toBe(false)
    expect(share.error.value).toBe("Couldn't copy the link.")
    writeText.mockResolvedValue(undefined)
    await share.copy()
    expect(share.copied.value).toBe(true)
    expect(share.error.value).toBeNull()
  })
})
