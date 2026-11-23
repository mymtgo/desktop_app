import { describe, expect, it } from 'vitest'
import type { DraftNotePickView, DraftNotesView } from '@/bindings'
import {
  afterNewView,
  clock,
  headerLabel,
  headerStatus,
  isUrgent,
  LIVE,
  navigation,
  notePlaceholder,
  pickStep,
  secondsLeft,
  step,
  URGENT_SECONDS,
  type Selection,
} from './draftNotesWindow'

const NOW = 1_787_397_120_000

function pick(ordinal: number, overrides: Partial<DraftNotePickView> = {}): DraftNotePickView {
  return {
    ordinal,
    label: `P1p${ordinal}`,
    cardsInPack: 15 - ordinal,
    deadlineAtMs: null,
    pickedCatalogId: 154000 + ordinal,
    pickedName: `Qcard ${ordinal}`,
    note: null,
    noteSavedAtMs: null,
    ...overrides,
  }
}

function notes(count: number, overrides: Partial<DraftNotesView> = {}): DraftNotesView {
  const picks: DraftNotePickView[] = []
  for (let ordinal = 1; ordinal <= count; ordinal += 1) picks.push(pick(ordinal))
  return {
    draftToken: 'q-draft-1',
    runKey: null,
    state: 'picking',
    currentOrdinal: count > 0 ? count : null,
    picks,
    ...overrides,
  }
}

describe('header label', () => {
  it('says when there is no draft', () => {
    expect(headerLabel({ hasDraft: false, state: null, pick: null, secondsLeft: null })).toBe('No draft in progress')
  })

  it('says joining while connecting and waiting before the first pack', () => {
    expect(headerLabel({ hasDraft: true, state: 'connecting', pick: null, secondsLeft: null })).toBe('Joining draft')
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: null, secondsLeft: null })).toBe('Waiting for pack')
    expect(headerLabel({ hasDraft: true, state: 'finished', pick: null, secondsLeft: null })).toBe('Waiting for pack')
  })

  it('names a picked card, or its catalog id without card data', () => {
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: pick(3), secondsLeft: null })).toBe('P1p3 · Qcard 3')
    const unknown = pick(4, { pickedCatalogId: 999999, pickedName: null })
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: unknown, secondsLeft: null })).toBe('P1p4 · #999999')
  })

  it('shows the pack size and the time left of the pick on the table', () => {
    const open = pick(3, { pickedCatalogId: null, pickedName: null, cardsInPack: 12 })
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: open, secondsLeft: 41 })).toBe(
      'P1p3 · 12 cards · 0:41 left',
    )
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: open, secondsLeft: 65 })).toBe(
      'P1p3 · 12 cards · 1:05 left',
    )
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: open, secondsLeft: null })).toBe('P1p3 · 12 cards')
    const empty = { ...open, cardsInPack: 0 }
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: empty, secondsLeft: 9 })).toBe('P1p3 · 0:09 left')
    expect(headerLabel({ hasDraft: true, state: 'picking', pick: empty, secondsLeft: null })).toBe('P1p3')
  })

  it('reads as a clock under a minute too', () => {
    expect(clock(0)).toBe('0:00')
    expect(clock(41)).toBe('0:41')
    expect(clock(600)).toBe('10:00')
  })
})

describe('urgency', () => {
  const open = pick(3, { pickedCatalogId: null, pickedName: null })

  it('turns red at 10 seconds or less while unpicked', () => {
    expect(URGENT_SECONDS).toBe(10)
    expect(isUrgent(11, open)).toBe(false)
    expect(isUrgent(10, open)).toBe(true)
    expect(isUrgent(0, open)).toBe(true)
  })

  it('never for a picked card or without a clock', () => {
    expect(isUrgent(5, pick(3))).toBe(false)
    expect(isUrgent(null, open)).toBe(false)
    expect(isUrgent(5, null)).toBe(false)
  })
})

describe('countdown', () => {
  const open = pick(3, { pickedCatalogId: null, pickedName: null, deadlineAtMs: NOW + 41_000 })

  it('ticks from the deadline on the local clock', () => {
    expect(secondsLeft(open, true, NOW)).toBe(41)
    expect(secondsLeft(open, true, NOW + 1_000)).toBe(40)
    expect(secondsLeft(open, true, NOW + 40_600)).toBe(0)
    expect(secondsLeft(open, true, NOW + 90_000)).toBe(0)
  })

  it('hides on a pinned past pick and without a deadline', () => {
    expect(secondsLeft(open, false, NOW)).toBeNull()
    expect(secondsLeft({ ...open, deadlineAtMs: null }, true, NOW)).toBeNull()
    expect(secondsLeft(null, true, NOW)).toBeNull()
  })
})

describe('pinning', () => {
  it('follows the newest pick', () => {
    const nav = navigation(notes(4), LIVE)
    expect(nav.pick?.ordinal).toBe(4)
    expect(nav.isLive).toBe(true)
    expect(nav.picksAhead).toBe(0)
    expect(nav.canPrev).toBe(true)
    expect(nav.canNext).toBe(false)
  })

  it('pins an earlier pick and counts the picks ahead for the Live chip', () => {
    let selection: Selection = step(notes(4), LIVE, -1)
    selection = step(notes(4), selection, -1)
    expect(selection).toEqual({ following: false, pinnedOrdinal: 2 })
    const nav = navigation(notes(4), selection)
    expect(nav.pick?.ordinal).toBe(2)
    expect(nav.isLive).toBe(false)
    expect(nav.picksAhead).toBe(2)
    expect(nav.canPrev).toBe(true)
    expect(nav.canNext).toBe(true)
  })

  it('stays on a pinned pick when a new pack lands', () => {
    const pinned = step(notes(4), LIVE, -1)
    const nav = navigation(notes(5), pinned)
    expect(nav.pick?.ordinal).toBe(3)
    expect(nav.picksAhead).toBe(2)
  })

  it('stepping onto the newest pick is live again', () => {
    const pinned = step(notes(4), LIVE, -1)
    const back = step(notes(4), pinned, 1)
    expect(back.following).toBe(true)
    expect(navigation(notes(4), back).isLive).toBe(true)
  })

  it('does not step past either end', () => {
    expect(step(notes(4), LIVE, 1)).toEqual(LIVE)
    const first = { following: false, pinnedOrdinal: 1 }
    expect(step(notes(4), first, -1)).toEqual(first)
    expect(navigation(notes(4), first).canPrev).toBe(false)
  })

  it('has nothing to show without picks', () => {
    const nav = navigation(notes(0), LIVE)
    expect(nav.pick).toBeNull()
    expect(nav.canPrev).toBe(false)
    expect(nav.canNext).toBe(false)
    expect(navigation(null, LIVE).pick).toBeNull()
  })

  it('a new draft resets to live', () => {
    const pinned: Selection = { following: false, pinnedOrdinal: 2 }
    const next = notes(1, { draftToken: 'q-draft-2' })
    expect(afterNewView(pinned, notes(4), next)).toEqual(LIVE)
    expect(afterNewView(pinned, notes(4), null)).toEqual(LIVE)
    // The same draft keeps the pin.
    expect(afterNewView(pinned, notes(4), notes(5))).toEqual(pinned)
  })
})

describe('keys', () => {
  const key = (key: string, mods: Partial<{ altKey: boolean; ctrlKey: boolean; metaKey: boolean }> = {}) => ({
    key,
    altKey: false,
    ctrlKey: false,
    metaKey: false,
    ...mods,
  })

  it('Alt+← and Alt+→ step through the picks', () => {
    expect(pickStep(key('ArrowLeft', { altKey: true }))).toBe(-1)
    expect(pickStep(key('ArrowRight', { altKey: true }))).toBe(1)
  })

  it('only with Alt, so a bare arrow moves the caret', () => {
    expect(pickStep(key('ArrowLeft'))).toBeNull()
    expect(pickStep(key('ArrowRight'))).toBeNull()
    expect(pickStep(key('ArrowLeft', { altKey: true, ctrlKey: true }))).toBeNull()
    expect(pickStep(key('ArrowRight', { altKey: true, metaKey: true }))).toBeNull()
    expect(pickStep(key('ArrowUp', { altKey: true }))).toBeNull()
  })
})

describe('note editor text', () => {
  it('asks why once a pack is on the table, unlinked drafts included', () => {
    expect(notePlaceholder(true)).toBe('Why this pick?')
    expect(notePlaceholder(false)).toBe('Notes appear here once a pack is on the table.')
  })

  it('shows the save states as 0.x', () => {
    expect(headerStatus('saving')).toEqual({ icon: 'spinner', text: null })
    expect(headerStatus('saved')).toEqual({ icon: 'check', text: 'saved' })
    expect(headerStatus('dirty')).toEqual({ icon: null, text: 'unsaved' })
    expect(headerStatus('error')).toEqual({ icon: null, text: 'not saved' })
    expect(headerStatus('idle')).toEqual({ icon: null, text: null })
  })
})
