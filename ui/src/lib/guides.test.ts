import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { GuideCardInputView } from '@/bindings'
import {
  balance,
  buildPlan,
  canStepUp,
  defaultDeck,
  groupByType,
  planSummary,
  postboardText,
  recordCell,
  recordLine,
  saveLabel,
  selectionTotals,
  sidedInText,
  staleLabel,
  stepPlanned,
  useCreateGuide,
  useGuideAutosave,
  youCutText,
} from './guides'

describe('the plan summary (0.x DeckSideboardGuides)', () => {
  it('says no cards are planned for an empty plan', () => {
    expect(planSummary(0, 0)).toBe('No cards planned')
  })

  it('sums the copies in and out', () => {
    expect(planSummary(3, 2)).toBe('+3 in / -2 out')
    expect(planSummary(1, 0)).toBe('+1 in / -0 out')
  })

  it('shows "-" in an empty cell, not 0.x em dash', () => {
    expect(recordCell(0, null)).toBe('-')
    expect(recordCell(3, '2 - 1')).toBe('2 - 1')
  })

  it('heads the editor with the record or "Not faced yet"', () => {
    expect(recordLine({ matches: 0, matchRecord: null, matchWinrate: null, gameWinrate: null })).toBe('Not faced yet')
    expect(recordLine({ matches: 3, matchRecord: '2 - 1', matchWinrate: 67, gameWinrate: 60 })).toBe(
      '2 - 1 · 67% match · 60% game',
    )
  })

  it('writes the card columns and the footer as 0.x', () => {
    expect(sidedInText({ sidedInGames: 0, wins: 0, losses: 0, winrate: null })).toBe('-')
    expect(sidedInText({ sidedInGames: 4, wins: 3, losses: 1, winrate: 75 })).toBe('3–1 (75%)')
    expect(youCutText({ sidedOutGames: 0 })).toBe('-')
    expect(youCutText({ sidedOutGames: 2 })).toBe('2×')
    expect(postboardText(0, '0 - 0')).toBe('No post-board games vs this archetype yet')
    expect(postboardText(5, '3 - 2')).toBe('5 post-board games · 3 - 2 overall')
  })
})

describe('the balance chip (0.x SideboardGuideBalance)', () => {
  it('is balanced when everything brought in is paid for', () => {
    expect(balance(2, 2, 15)).toEqual({ resulting: 15, hint: null })
  })

  it('asks to cut more when more comes in than goes out', () => {
    expect(balance(3, 1, 15)).toEqual({ resulting: 13, hint: 'cut 2 more' })
  })

  it('asks to bring in more when more goes out than comes in', () => {
    expect(balance(0, 2, 15)).toEqual({ resulting: 17, hint: 'bring in 2 more' })
  })
})

describe('the steppers (0.x SideboardGuideCardRow)', () => {
  it('stop at the zone copies and at zero', () => {
    expect(stepPlanned(1, 2, false, 1)).toBe(2)
    expect(stepPlanned(2, 2, false, 1)).toBe(2)
    expect(stepPlanned(0, 2, false, -1)).toBe(0)
    expect(canStepUp(1, 2, false)).toBe(true)
    expect(canStepUp(2, 2, false)).toBe(false)
  })

  it('let a stale row only go down', () => {
    expect(stepPlanned(2, 0, true, 1)).toBe(2)
    expect(stepPlanned(2, 0, true, -1)).toBe(1)
    expect(canStepUp(1, 4, true)).toBe(false)
    expect(staleLabel('in')).toBe('Not in sideboard')
    expect(staleLabel('out')).toBe('Not in maindeck')
  })

  it('send the plan by direction then oracle id, without zeros', () => {
    expect(buildPlan({ 'out:o-b': 1, 'in:o-z': 2, 'in:o-a': 1, 'in:o-gone': 0 })).toEqual([
      { oracleId: 'o-a', direction: 'in', quantity: 1 },
      { oracleId: 'o-z', direction: 'in', quantity: 2 },
      { oracleId: 'o-b', direction: 'out', quantity: 1 },
    ])
    expect(selectionTotals({ 'in:o-a': 2, 'in:o-b': 1, 'out:o-c': 3 })).toEqual({ totalIn: 3, totalOut: 3 })
  })
})

describe('the autosave (0.x SideboardGuideEdit)', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  /** A save that resolves only when told. */
  function deferredSave() {
    const calls: { cards: GuideCardInputView[]; resolve: (message: string | null) => void }[] = []
    const save = (cards: GuideCardInputView[]) =>
      new Promise<string | null>((resolve) => {
        calls.push({ cards, resolve })
      })
    return { calls, save }
  }

  it('waits 500 ms after the last change, then sends the whole plan once', async () => {
    const { calls, save } = deferredSave()
    const autosave = useGuideAutosave(save)
    autosave.reset({ 'in:o-rip': 1 })

    autosave.set('in', 'o-rip', 2)
    vi.advanceTimersByTime(300)
    autosave.set('out', 'o-bolt', 1)
    expect(autosave.label.value).toBe('Saving…')
    vi.advanceTimersByTime(499)
    expect(calls).toHaveLength(0)
    vi.advanceTimersByTime(1)
    expect(calls).toHaveLength(1)
    expect(calls[0].cards).toEqual([
      { oracleId: 'o-rip', direction: 'in', quantity: 2 },
      { oracleId: 'o-bolt', direction: 'out', quantity: 1 },
    ])

    calls[0].resolve(null)
    await vi.runAllTimersAsync()
    expect(autosave.state.value).toBe('saved')
    expect(saveLabel(autosave.state.value)).toBe('Saved')
  })

  it('saves again when a change lands during a save', async () => {
    const { calls, save } = deferredSave()
    const autosave = useGuideAutosave(save)
    autosave.reset({})

    autosave.set('in', 'o-rip', 1)
    vi.advanceTimersByTime(500)
    expect(calls).toHaveLength(1)
    autosave.set('in', 'o-rip', 2)
    vi.advanceTimersByTime(500)
    // Still running: the second change waits for it.
    expect(calls).toHaveLength(1)

    calls[0].resolve(null)
    await vi.runAllTimersAsync()
    expect(calls).toHaveLength(2)
    expect(calls[1].cards).toEqual([{ oracleId: 'o-rip', direction: 'in', quantity: 2 }])
    calls[1].resolve(null)
    await vi.runAllTimersAsync()
    expect(autosave.state.value).toBe('saved')
  })

  it('sends nothing when the plan is back to what was saved', async () => {
    const { calls, save } = deferredSave()
    const autosave = useGuideAutosave(save)
    autosave.reset({ 'in:o-rip': 1 })

    autosave.set('in', 'o-rip', 2)
    autosave.set('in', 'o-rip', 1)
    vi.advanceTimersByTime(500)
    expect(calls).toHaveLength(0)
    expect(autosave.state.value).toBe('saved')
  })

  it('reverts to the saved plan and shows the first message when refused', async () => {
    const { calls, save } = deferredSave()
    const autosave = useGuideAutosave(save)
    autosave.reset({ 'in:o-rip': 1 })

    autosave.set('in', 'o-rip', 2)
    vi.advanceTimersByTime(500)
    calls[0].resolve(null)
    await vi.runAllTimersAsync()
    autosave.set('in', 'o-rip', 3)
    vi.advanceTimersByTime(500)
    calls[1].resolve('Only 2 in the sideboard.')
    await vi.runAllTimersAsync()

    expect(autosave.state.value).toBe('error')
    expect(autosave.label.value).toBe('Not saved')
    expect(autosave.error.value).toBe('Only 2 in the sideboard.')
    expect({ ...autosave.selection }).toEqual({ 'in:o-rip': 2 })
    expect(calls).toHaveLength(2)
  })

  it('shows 0.x message and keeps the plan when the save itself fails, sending it again on the next change', async () => {
    const save = vi.fn().mockRejectedValueOnce(new Error('tracking is not running')).mockResolvedValue(null)
    const autosave = useGuideAutosave(save)
    autosave.reset({})

    autosave.set('out', 'o-bolt', 1)
    vi.advanceTimersByTime(500)
    await vi.runAllTimersAsync()
    expect(autosave.error.value).toBe('Could not save your changes.')
    expect({ ...autosave.selection }).toEqual({ 'out:o-bolt': 1 })

    autosave.set('in', 'o-rip', 1)
    await vi.runAllTimersAsync()
    expect(save).toHaveBeenCalledTimes(2)
    expect(autosave.state.value).toBe('saved')
  })

  // #300 F14: a change made while a save fails is sent once that save
  // ends, not held until the next change; a plan that failed as sent is
  // not sent again on its own.
  it('sends a change made during a failing save once it ends', async () => {
    const calls: { cards: GuideCardInputView[]; reject: (error: Error) => void; resolve: (message: string | null) => void }[] = []
    const save = (cards: GuideCardInputView[]) =>
      new Promise<string | null>((resolve, reject) => {
        calls.push({ cards, resolve, reject })
      })
    const autosave = useGuideAutosave(save)
    autosave.reset({})

    autosave.set('in', 'o-rip', 1)
    vi.advanceTimersByTime(500)
    autosave.set('in', 'o-rip', 2)
    vi.advanceTimersByTime(500)
    expect(calls).toHaveLength(1)

    calls[0].reject(new Error('tracking is restarting'))
    await vi.runAllTimersAsync()
    expect(calls).toHaveLength(2)
    expect(calls[1].cards).toEqual([{ oracleId: 'o-rip', direction: 'in', quantity: 2 }])

    calls[1].reject(new Error('tracking is restarting'))
    await vi.runAllTimersAsync()
    expect(calls).toHaveLength(2)
    expect(autosave.state.value).toBe('error')
  })

  it('sends a waiting change straight away when the page is left', () => {
    const { calls, save } = deferredSave()
    const autosave = useGuideAutosave(save)
    autosave.reset({})

    autosave.set('in', 'o-rip', 1)
    autosave.leave()
    expect(calls).toHaveLength(1)
  })
})

describe('the create dialog (0.x CreateSideboardGuideDialog)', () => {
  it('closes on success with the new guide', async () => {
    const dialog = useCreateGuide(async () => ({ status: 'ok', data: 7 }))
    dialog.show()
    expect(await dialog.select(3)).toBe(7)
    expect(dialog.open.value).toBe(false)
    expect(dialog.error.value).toBe(null)
  })

  it('keeps the duplicate message inline and stays open', async () => {
    const dialog = useCreateGuide(async () => ({
      status: 'error',
      error: { kind: 'refused', message: 'A guide for this archetype already exists for this deck.' },
    }))
    dialog.show()
    expect(await dialog.select(3)).toBe(null)
    expect(dialog.open.value).toBe(true)
    expect(dialog.error.value).toBe('A guide for this archetype already exists for this deck.')
    expect(dialog.processing.value).toBe(false)

    // Reopening starts clean.
    dialog.show()
    expect(dialog.error.value).toBe(null)
  })
})

describe('the archetype page block', () => {
  it('defaults the deck select to the deck most played against it', () => {
    expect(
      defaultDeck([
        { deckId: 4, name: 'Qoften', matches: 5 },
        { deckId: 2, name: 'Qrarely', matches: 1 },
      ]),
    ).toBe(4)
  })

  it('has no default without a deck of the format', () => {
    expect(defaultDeck([])).toBe(null)
  })
})

describe('the editor columns', () => {
  it('group rows by card type in 0.x order, unknown types as Other', () => {
    const rows = [
      { name: 'Qland', typeLine: 'Basic Land' },
      { name: 'Qbolt', typeLine: 'Instant' },
      { name: 'Qbeast', typeLine: 'Legendary Creature - Beast' },
      { name: 'Qunknown', typeLine: null },
    ]
    expect(groupByType(rows).map((group) => [group.type, group.rows.map((row) => row.name)])).toEqual([
      ['Creature', ['Qbeast']],
      ['Instant', ['Qbolt']],
      ['Land', ['Qland']],
      ['Other', ['Qunknown']],
    ])
  })
})
