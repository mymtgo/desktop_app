import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Toast } from '@/composables/useToast'

const invalidateQueries = vi.fn()
const add = vi.fn()
const setMatchNotes = vi.fn()
const hideMatch = vi.fn()
const restoreMatch = vi.fn()
const moveMatchToLeague = vi.fn()
const createManualMatch = vi.fn()
const setManualHand = vi.fn()
const setManualSideboard = vi.fn()
const setManualReveals = vi.fn()
const setMatchDeck = vi.fn()
const correctMatch = vi.fn()
const matchEditView = vi.fn()

vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({ invalidateQueries }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))
vi.mock('@/bindings', () => ({
  commands: {
    setMatchNotes,
    hideMatch,
    restoreMatch,
    moveMatchToLeague,
    createManualMatch,
    setManualHand,
    setManualSideboard,
    setManualReveals,
    setMatchDeck,
    correctMatch,
    matchEditView,
  },
}))

const { useMatchEdit } = await import('./useMatchEdit')

describe('useMatchEdit', () => {
  beforeEach(() => {
    invalidateQueries.mockReset()
    invalidateQueries.mockResolvedValue(undefined)
    add.mockReset()
    setMatchNotes.mockReset()
    hideMatch.mockReset()
    restoreMatch.mockReset()
    moveMatchToLeague.mockReset()
    createManualMatch.mockReset()
    setManualHand.mockReset()
    setManualSideboard.mockReset()
    setManualReveals.mockReset()
    setMatchDeck.mockReset()
    correctMatch.mockReset()
    matchEditView.mockReset()
  })

  it('saves notes and refetches every match list, as notes push no event', async () => {
    setMatchNotes.mockResolvedValue({ status: 'ok', data: null })
    const { saveNotes, saving } = useMatchEdit()

    const pending = saveNotes('QTOKEN', 'Sided in the wrong plan')
    expect(saving.value).toBe(true)
    expect(await pending).toBe(true)

    expect(saving.value).toBe(false)
    expect(setMatchNotes).toHaveBeenCalledWith('QTOKEN', 'Sided in the wrong plan')
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['matches'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['decks', 'matches'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['dashboard'] })
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['matchDetail'] })
    // The Limited pages list matches too (their notes icon, the match page).
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['limited'] })
    expect(add).not.toHaveBeenCalled()
  })

  it('deletes a match with an Undo toast that restores it', async () => {
    hideMatch.mockResolvedValue({ status: 'ok', data: null })
    restoreMatch.mockResolvedValue({ status: 'ok', data: null })
    const { hide } = useMatchEdit()

    expect(await hide('QTOKEN')).toBe(true)
    expect(hideMatch).toHaveBeenCalledWith('QTOKEN')
    const toast = add.mock.calls[0][0] as Toast
    expect(toast.title).toBe('Match deleted')
    expect(toast.message).toBe('You can restore it from Settings, Archives.')
    expect(toast.action?.label).toBe('Undo')
    toast.action?.run()
    await vi.waitFor(() => expect(restoreMatch).toHaveBeenCalledWith('QTOKEN'))
    await vi.waitFor(() => expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['leagues'] }))
  })

  it('moves a match into a league and names where it came from', async () => {
    moveMatchToLeague.mockResolvedValue({ status: 'ok', data: null })
    const { moveToLeague } = useMatchEdit()
    const from = { key: 'q:old', name: 'Qold', startedAtMs: Date.UTC(2026, 9, 11, 12) }

    expect(await moveToLeague('QTOKEN', 'q:new', from)).toBe(true)
    expect(moveMatchToLeague).toHaveBeenCalledWith('QTOKEN', 'q:new')
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ['leagues'] })
    expect(add.mock.calls[0][0]).toMatchObject({ type: 'success', title: 'Match moved' })
  })

  it('takes a match out of every league', async () => {
    moveMatchToLeague.mockResolvedValue({ status: 'ok', data: null })
    const { moveToLeague } = useMatchEdit()
    expect(await moveToLeague('QTOKEN', null, null)).toBe(true)
    expect(moveMatchToLeague).toHaveBeenCalledWith('QTOKEN', null)
    expect(add.mock.calls[0][0]).toMatchObject({ title: 'Removed from its league' })
  })

  it('shows a refused move as a toast', async () => {
    moveMatchToLeague.mockResolvedValue({
      status: 'error',
      error: { kind: 'refused', message: 'Tournament matches stay in their tournament' },
    })
    const { moveToLeague } = useMatchEdit()
    expect(await moveToLeague('QTOKEN', 'q:new', null)).toBe(false)
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not move the match',
      message: 'Tournament matches stay in their tournament',
    })
  })

  it('shows a refusal as a toast and refetches nothing', async () => {
    setMatchNotes.mockResolvedValue({
      status: 'error',
      error: { kind: 'refused', message: 'The notes field must not be greater than 5000 characters.' },
    })
    const { saveNotes } = useMatchEdit()

    expect(await saveNotes('QTOKEN', 'x'.repeat(5001))).toBe(false)
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not save notes',
      message: 'The notes field must not be greater than 5000 characters.',
    })
  })

  const input = {
    deckId: 3,
    opponentName: 'Qorvath',
    league: null,
    startedAt: '2026-10-08T19:30',
    endedAt: '2026-10-08T20:15',
    games: [{ won: true, onPlay: true, turns: null }],
  }

  function expectEverythingRefetched() {
    for (const key of [['matches'], ['decks'], ['leagues'], ['dashboard'], ['matchDetail'], ['limited']]) {
      expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: key })
    }
  }

  it('adds a manual match, shows its flash and refetches matches, decks, leagues and the dashboard', async () => {
    createManualMatch.mockResolvedValue({
      status: 'ok',
      data: { kind: 'created', token: 'QNEW', message: 'Manual match added.' },
    })
    const { createManual } = useMatchEdit()

    expect(await createManual(input)).toEqual({ ok: true, token: 'QNEW' })
    expect(createManualMatch).toHaveBeenCalledWith(input)
    expectEverythingRefetched()
    expect(add).toHaveBeenCalledWith({ type: 'success', title: 'Success', message: 'Manual match added.' })
  })

  it('hands field errors back to the form and refetches nothing', async () => {
    createManualMatch.mockResolvedValue({
      status: 'ok',
      data: { kind: 'invalid', errors: { opponent_name: 'The opponent name field is required.' } },
    })
    const { createManual } = useMatchEdit()

    expect(await createManual(input)).toEqual({
      ok: false,
      errors: { opponent_name: 'The opponent name field is required.' },
    })
    expect(invalidateQueries).not.toHaveBeenCalled()
    expect(add).not.toHaveBeenCalled()
  })

  it('shows the sideboard flash as a toast, the cleared hand included', async () => {
    const message = 'Sideboard saved. Opening hand cleared because it used a card you sided out.'
    setManualSideboard.mockResolvedValue({ status: 'ok', data: { kind: 'saved', message } })
    const { saveManualSideboard } = useMatchEdit()
    const changes = [{ catalogId: 1, quantity: 1, kind: 'out' as const }]

    expect(await saveManualSideboard('QTOKEN', 2, changes)).toEqual({ ok: true })
    expect(setManualSideboard).toHaveBeenCalledWith('QTOKEN', 2, changes)
    expect(add).toHaveBeenCalledWith({ type: 'success', title: 'Success', message })
    expectEverythingRefetched()
  })

  it('saves a hand and reveals through the tracker', async () => {
    setManualHand.mockResolvedValue({ status: 'ok', data: { kind: 'saved', message: 'Opening hand saved.' } })
    setManualReveals.mockResolvedValue({ status: 'ok', data: { kind: 'saved', message: 'Revealed cards saved.' } })
    const { saveManualHand, saveManualReveals } = useMatchEdit()
    const hand = { mulliganCount: 0, startingHand: [1, 1, 1, 1, 2, 2, 2], bottomed: [], mulliganedHands: [] }

    expect(await saveManualHand('QTOKEN', 1, hand)).toEqual({ ok: true })
    expect(setManualHand).toHaveBeenCalledWith('QTOKEN', 1, hand)
    expect(await saveManualReveals('QTOKEN', 1, [{ catalogId: 5, quantity: 2 }])).toEqual({ ok: true })
    expect(setManualReveals).toHaveBeenCalledWith('QTOKEN', 1, [{ catalogId: 5, quantity: 2 }])
  })

  it('shows a refused edit as a toast', async () => {
    setManualHand.mockResolvedValue({
      status: 'error',
      error: { kind: 'refused', message: 'That game does not exist.' },
    })
    const { saveManualHand } = useMatchEdit()

    expect(
      await saveManualHand('QTOKEN', 4, { mulliganCount: 0, startingHand: [], bottomed: [], mulliganedHands: [] }),
    ).toEqual({ ok: false, errors: {} })
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not save the hand',
      message: 'That game does not exist.',
    })
    expect(invalidateQueries).not.toHaveBeenCalled()
  })

  // Issue #332: "Set deck" on the Matches page.
  it('sets the deck of matches and refetches matches, decks, leagues, the dashboard and match pages', async () => {
    setMatchDeck.mockResolvedValue({ status: 'ok', data: null })
    const { setDeck } = useMatchEdit()

    expect(await setDeck(['QTOKEN', 'QOTHER'], 12)).toBe(true)
    expect(setMatchDeck).toHaveBeenCalledWith(['QTOKEN', 'QOTHER'], 12)
    for (const queryKey of [['matches'], ['decks'], ['leagues'], ['dashboard'], ['matchDetail']]) {
      expect(invalidateQueries).toHaveBeenCalledWith({ queryKey })
    }
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ type: 'success' }))
  })

  it('shows a refused deck as a toast and refetches nothing', async () => {
    setMatchDeck.mockResolvedValue({
      status: 'error',
      error: { kind: 'refused', message: 'That deck version no longer exists. Choose another one.' },
    })
    const { setDeck } = useMatchEdit()

    expect(await setDeck(['QTOKEN'], 12)).toBe(false)
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not set the deck',
      message: 'That deck version no longer exists. Choose another one.',
    })
    expect(invalidateQueries).not.toHaveBeenCalled()
  })
  it('saves a fix and refetches every query but the pushed status and settings', async () => {
    correctMatch.mockResolvedValue({ status: 'ok', data: { kind: 'saved', message: 'Match updated.' } })
    const { correctMatch: correct } = useMatchEdit()
    const input = { opponentName: 'Qrival', startedAt: '2026-10-16T20:05', endedAt: null, games: [] }

    expect(await correct(7, input)).toEqual({ ok: true })
    expect(correctMatch).toHaveBeenCalledWith(7, input)
    const [{ predicate }] = invalidateQueries.mock.calls[0] as [{ predicate: (q: { queryKey: unknown[] }) => boolean }]
    for (const key of ['matches', 'decks', 'leagues', 'dashboard', 'matchDetail', 'limited', 'metagame', 'cardsPage']) {
      expect(predicate({ queryKey: [key] })).toBe(true)
    }
    expect(predicate({ queryKey: ['status'] })).toBe(false)
    expect(predicate({ queryKey: ['settings'] })).toBe(false)
    expect(add).toHaveBeenCalledWith({ type: 'success', title: 'Success', message: 'Match updated.' })
  })

  it('answers a refused fix with its field errors and refetches nothing', async () => {
    correctMatch.mockResolvedValue({
      status: 'ok',
      data: { kind: 'invalid', errors: { started_at: 'The started at field is required.' } },
    })
    const { correctMatch: correct } = useMatchEdit()
    const input = { opponentName: null, startedAt: '', endedAt: null, games: [] }
    expect(await correct(7, input)).toEqual({
      ok: false,
      errors: { started_at: 'The started at field is required.' },
    })
    expect(invalidateQueries).not.toHaveBeenCalled()
  })

  it('loads the edit view, or shows a toast when the match is gone', async () => {
    const view = { opponentName: null, startedAtMs: 1, endedAtMs: null, games: [], corrected: false }
    matchEditView.mockResolvedValueOnce({ status: 'ok', data: view })
    const { loadEditView } = useMatchEdit()
    expect(await loadEditView(7)).toEqual(view)
    matchEditView.mockResolvedValueOnce({
      status: 'error',
      error: { kind: 'refused', message: 'This match no longer exists.' },
    })
    expect(await loadEditView(8)).toBeNull()
    expect(add).toHaveBeenCalledWith({
      type: 'error',
      title: 'Could not open the match',
      message: 'This match no longer exists.',
    })
  })
})
