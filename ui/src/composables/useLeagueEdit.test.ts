import { beforeEach, describe, expect, it, vi } from 'vitest'

const invalidateQueries = vi.fn()
const add = vi.fn()

vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({ invalidateQueries }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))

const { useLeagueEdit } = await import('./useLeagueEdit')

describe('useLeagueEdit', () => {
  beforeEach(() => {
    invalidateQueries.mockReset()
    invalidateQueries.mockResolvedValue(undefined)
    add.mockReset()
  })

  it('refreshes every league and deck query after an edit succeeds', async () => {
    const { run, saving } = useLeagueEdit()
    let savingDuring = false
    const done = await run('Could not drop the league', async () => {
      savingDuring = saving.value
      return { status: 'ok', data: null }
    })

    expect(done).toBe(true)
    expect(savingDuring).toBe(true)
    expect(saving.value).toBe(false)
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['leagues'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['decks'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['limited'] })
    expect(add).not.toHaveBeenCalled()
  })

  it('shows a refusal as a toast and refreshes nothing', async () => {
    const { run } = useLeagueEdit()
    const done = await run('Could not save the notes', async () => ({
      status: 'error',
      error: { kind: 'refused', message: 'That league was deleted' },
    }))

    expect(done).toBe(false)
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not save the notes',
      message: 'That league was deleted',
    })
  })

  it('hands a refusal back without a toast when asked to attempt', async () => {
    const { attempt } = useLeagueEdit()
    const outcome = await attempt(async () => ({
      status: 'error',
      error: { kind: 'refused', message: 'The name field is required.' },
    }))

    expect(outcome).toEqual({ ok: false, message: 'The name field is required.' })
    expect(add).not.toHaveBeenCalled()
    expect(invalidateQueries).not.toHaveBeenCalled()
  })
})
