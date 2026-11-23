import type { DraftNotePickView, DraftNotesView } from '@/bindings'
import type { NoteStatus } from '@/lib/limited'

/**
 * The draft notes window's rules (overlay spec 8; 0.x `DraftNotes.vue` and
 * `DraftNotesHeader.vue`): which pick the editor shows, the header label, the
 * countdown and the keys.
 */

/** The header turns red at this many seconds left, or fewer (0.x). */
export const URGENT_SECONDS = 10

/**
 * Which pick the editor points at. The window follows the newest pick;
 * stepping back pins one pick, so a pack landing mid-note never drags the
 * editor away from what you were writing about.
 */
export type Selection = { following: boolean; pinnedOrdinal: number | null }

export const LIVE: Selection = { following: true, pinnedOrdinal: null }

export type PickNavigation = {
  pick: DraftNotePickView | null
  /** The editor shows the pick on the table. */
  isLive: boolean
  /** Picks after the pinned one, for the "Live +{n}" chip. */
  picksAhead: number
  canPrev: boolean
  canNext: boolean
}

function selectedOrdinal(notes: DraftNotesView | null, selection: Selection): number | null {
  const current = notes?.currentOrdinal ?? null
  return selection.following ? current : selection.pinnedOrdinal
}

function selectedIndex(notes: DraftNotesView | null, selection: Selection): number {
  const ordinal = selectedOrdinal(notes, selection)
  return (notes?.picks ?? []).findIndex((pick) => pick.ordinal === ordinal)
}

/** Where the editor is in the draft. */
export function navigation(notes: DraftNotesView | null, selection: Selection): PickNavigation {
  const picks = notes?.picks ?? []
  const ordinal = selectedOrdinal(notes, selection)
  const index = selectedIndex(notes, selection)
  const isLive = ordinal !== null && ordinal === (notes?.currentOrdinal ?? null)
  return {
    pick: picks[index] ?? null,
    isLive,
    picksAhead: isLive || index < 0 ? 0 : picks.length - 1 - index,
    canPrev: index > 0,
    canNext: index >= 0 && index < picks.length - 1,
  }
}

/**
 * One pick back (-1) or forward (1). Stepping onto the newest pick means
 * live again, so › and the Live chip end in the same place.
 */
export function step(notes: DraftNotesView | null, selection: Selection, delta: -1 | 1): Selection {
  const picks = notes?.picks ?? []
  const index = selectedIndex(notes, selection)
  if (index < 0) return selection
  const pick = picks[index + delta]
  if (!pick) return selection
  return { following: pick.ordinal === (notes?.currentOrdinal ?? null), pinnedOrdinal: pick.ordinal }
}

/** A new draft (or none) always starts live: a pin from the last one means nothing. */
export function afterNewView(
  selection: Selection,
  previous: DraftNotesView | null,
  next: DraftNotesView | null,
): Selection {
  if (next === null || previous?.draftToken !== next.draftToken) return LIVE
  return selection
}

/**
 * Seconds left on the pick on the table, from its deadline and the local
 * clock (the same machine wrote the deadline). A pinned past pick's deadline
 * is history and would read as a stuck 0:00, so it has none.
 */
export function secondsLeft(pick: DraftNotePickView | null, isLive: boolean, nowMs: number): number | null {
  if (!isLive || pick?.deadlineAtMs == null) return null
  return Math.max(0, Math.round((pick.deadlineAtMs - nowMs) / 1000))
}

/** `0:41`, `1:05`: the header always reads as a clock. */
export function clock(seconds: number): string {
  const minutes = Math.floor(seconds / 60)
  const rest = seconds % 60
  return `${minutes}:${String(rest).padStart(2, '0')}`
}

/** The header's label. */
export function headerLabel(input: {
  hasDraft: boolean
  state: string | null
  pick: DraftNotePickView | null
  secondsLeft: number | null
}): string {
  if (!input.hasDraft) return 'No draft in progress'
  const pick = input.pick
  if (!pick) return input.state === 'connecting' ? 'Joining draft' : 'Waiting for pack'
  if (pick.pickedCatalogId !== null) return `${pick.label} · ${pick.pickedName ?? `#${pick.pickedCatalogId}`}`
  const parts = [pick.label]
  if (pick.cardsInPack > 0) parts.push(`${pick.cardsInPack} cards`)
  if (input.secondsLeft !== null) parts.push(`${clock(input.secondsLeft)} left`)
  return parts.join(' · ')
}

/** Red while the pick on the table has 10 seconds or less. */
export function isUrgent(secondsLeft: number | null, pick: DraftNotePickView | null): boolean {
  return secondsLeft !== null && secondsLeft <= URGENT_SECONDS && pick !== null && pick.pickedCatalogId === null
}

/**
 * Alt+← and Alt+→ step through the picks. Alt rather than a bare arrow: the
 * text box holds focus for most of a draft, and a bare arrow moves the caret.
 */
export function pickStep(event: { key: string; altKey: boolean; ctrlKey: boolean; metaKey: boolean }): -1 | 1 | null {
  if (!event.altKey || event.ctrlKey || event.metaKey) return null
  if (event.key === 'ArrowLeft') return -1
  if (event.key === 'ArrowRight') return 1
  return null
}

/**
 * The text box's placeholder. Notes are kept per draft, so a draft not
 * linked to a league yet takes them too (spec ruling 12).
 */
export function notePlaceholder(canEdit: boolean): string {
  return canEdit ? 'Why this pick?' : 'Notes appear here once a pack is on the table.'
}

/** The header's save state (0.x): a spinner, a check with "saved", "unsaved", "not saved". */
export function headerStatus(status: NoteStatus): { icon: 'spinner' | 'check' | null; text: string | null } {
  if (status === 'saving') return { icon: 'spinner', text: null }
  if (status === 'saved') return { icon: 'check', text: 'saved' }
  if (status === 'error') return { icon: null, text: 'not saved' }
  if (status === 'dirty') return { icon: null, text: 'unsaved' }
  return { icon: null, text: null }
}
