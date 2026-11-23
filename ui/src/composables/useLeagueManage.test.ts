import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Toast } from '@/composables/useToast'

const invalidateQueries = vi.fn()
const add = vi.fn()
const setLeagueStatus = vi.fn()
const deleteLeague = vi.fn()
const undoDeleteLeague = vi.fn()
const restoreLeague = vi.fn()

vi.mock('@tanstack/vue-query', () => ({ useQueryClient: () => ({ invalidateQueries }) }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))
vi.mock('@/bindings', () => ({ commands: { setLeagueStatus, deleteLeague, undoDeleteLeague, restoreLeague } }))

const { useLeagueManage, UNDO_TOAST_MS } = await import('./useLeagueManage')

const OK = { status: 'ok', data: null }

describe('useLeagueManage', () => {
  beforeEach(() => {
    for (const mock of [invalidateQueries, add, setLeagueStatus, deleteLeague, undoDeleteLeague, restoreLeague]) {
      mock.mockReset()
    }
    invalidateQueries.mockResolvedValue(undefined)
  })

  it('sets a status and refetches the leagues, decks and matches', async () => {
    setLeagueStatus.mockResolvedValue(OK)
    const { setStatus } = useLeagueManage()
    expect(await setStatus('q:1', 'partial')).toBe(true)
    expect(setLeagueStatus).toHaveBeenCalledWith('q:1', 'partial')
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['leagues'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['decks'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['matches'] })
    expect(add).not.toHaveBeenCalled()
  })

  it('deletes with an Undo toast that puts the league and its status back', async () => {
    deleteLeague.mockResolvedValue({ status: 'ok', data: { leagueId: 7, removedAt: 1234, previousStatus: 'complete' } })
    undoDeleteLeague.mockResolvedValue({ status: 'ok', data: 'q:1' })
    setLeagueStatus.mockResolvedValue(OK)
    const { deleteLeague: remove } = useLeagueManage()

    expect(await remove('q:1', 'keep')).toBe(true)
    expect(deleteLeague).toHaveBeenCalledWith('q:1', 'keep')
    const toast = add.mock.calls[0][0] as Toast
    expect(toast.title).toBe('League deleted')
    expect(toast.message).toBe('You can restore it from Settings, Archives.')
    expect(toast.duration).toBe(UNDO_TOAST_MS)
    expect(toast.action?.label).toBe('Undo')

    toast.action?.run()
    await vi.waitFor(() => expect(setLeagueStatus).toHaveBeenCalledWith('q:1', 'complete'))
    expect(undoDeleteLeague).toHaveBeenCalledWith(7, 1234)
  })

  it('leaves the status alone on Undo when the delete recorded no drop', async () => {
    deleteLeague.mockResolvedValue({ status: 'ok', data: { leagueId: 7, removedAt: 99, previousStatus: null } })
    undoDeleteLeague.mockResolvedValue({ status: 'ok', data: 'q:1' })
    const { deleteLeague: remove } = useLeagueManage()
    await remove('q:1', 'delete')
    ;(add.mock.calls[0][0] as Toast).action?.run()
    await vi.waitFor(() => expect(undoDeleteLeague).toHaveBeenCalledWith(7, 99))
    expect(setLeagueStatus).not.toHaveBeenCalled()
  })

  it('puts the status back under the key Undo answers, after a rekey', async () => {
    deleteLeague.mockResolvedValue({ status: 'ok', data: { leagueId: 7, removedAt: 1234, previousStatus: 'partial' } })
    undoDeleteLeague.mockResolvedValue({ status: 'ok', data: 'q:2' })
    setLeagueStatus.mockResolvedValue(OK)
    const { deleteLeague: remove } = useLeagueManage()
    await remove('q:1', 'keep')
    ;(add.mock.calls[0][0] as Toast).action?.run()
    await vi.waitFor(() => expect(setLeagueStatus).toHaveBeenCalledWith('q:2', 'partial'))
    expect(undoDeleteLeague).toHaveBeenCalledWith(7, 1234)
  })

  it('shows a refusal as a toast and refetches nothing', async () => {
    deleteLeague.mockResolvedValue({ status: 'error', error: { kind: 'refused', message: 'Drop it first' } })
    const { deleteLeague: remove } = useLeagueManage()
    expect(await remove('q:1', 'keep')).toBe(false)
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(add).toHaveBeenCalledWith({ type: 'error', title: 'Could not delete the league', message: 'Drop it first' })
  })

  it('restores a league with a toast', async () => {
    restoreLeague.mockResolvedValue(OK)
    const { restoreLeague: restore } = useLeagueManage()
    expect(await restore('q:1')).toBe(true)
    expect(restoreLeague).toHaveBeenCalledWith('q:1')
    expect(add).toHaveBeenCalledWith({ type: 'success', title: 'League restored', message: 'Matches deleted with it are back too.' })
  })

  it('shows a refused restore as a toast: the league stays in Archives', async () => {
    const message = 'No matches are left in this league, so it has nothing to show.'
    restoreLeague.mockResolvedValue({ status: 'error', error: { kind: 'refused', message } })
    const { restoreLeague: restore } = useLeagueManage()
    expect(await restore('q:1')).toBe(false)
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(add).toHaveBeenCalledWith({ type: 'error', title: 'Could not restore the league', message })
  })
})
