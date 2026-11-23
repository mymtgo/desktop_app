import { beforeEach, describe, expect, it, vi } from 'vitest'

const invalidateQueries = vi.fn()
const add = vi.fn()

vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({ invalidateQueries }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))

const { useDeckEdit } = await import('./useDeckEdit')

describe('useDeckEdit', () => {
  beforeEach(() => {
    invalidateQueries.mockReset()
    invalidateQueries.mockResolvedValue(undefined)
    add.mockReset()
  })

  it('refreshes every deck query after an edit succeeds', async () => {
    const { run, saving } = useDeckEdit()
    let savingDuring = false
    const done = await run('Could not rename the deck', async () => {
      savingDuring = saving.value
      return { status: 'ok', data: null }
    })

    expect(done).toBe(true)
    expect(savingDuring).toBe(true)
    expect(saving.value).toBe(false)
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['decks'] })
    expect(add).not.toHaveBeenCalled()
  })

  it('shows a refusal as a toast and refreshes nothing', async () => {
    const { run, saving } = useDeckEdit()
    const done = await run('Could not rename the deck', async () => ({
      status: 'error',
      error: { kind: 'refused', message: 'This deck has been deleted on MTGO and is read-only.' },
    }))

    expect(done).toBe(false)
    expect(saving.value).toBe(false)
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not rename the deck',
      message: 'This deck has been deleted on MTGO and is read-only.',
    })
  })
})
