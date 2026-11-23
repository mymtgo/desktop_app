// Pure helpers for the deck Settings page (0.x `pages/decks/Settings.vue`)
// and the sidebar's click-to-rename name.

import type { CoverOptionView } from '@/bindings'

/** The longest deck name, in characters (the app's rule). */
export const DECK_NAME_MAX = 255

/**
 * A name input's `maxlength`. It counts UTF-16 units, two for an astral
 * character such as an emoji, so twice the limit never cuts a valid name
 * short; `deckNameProblem` enforces the real limit in characters.
 */
export const DECK_NAME_INPUT_MAX = DECK_NAME_MAX * 2

const NAME_MESSAGE = `A deck name needs 1 to ${DECK_NAME_MAX} characters.`

/** The typed text cut to `DECK_NAME_MAX` characters (code points, as the app counts). */
export function clampDeckName(text: string): string {
  const characters = [...text]
  return characters.length > DECK_NAME_MAX ? characters.slice(0, DECK_NAME_MAX).join('') : text
}

/** The text that confirms an archive (0.x typed DELETE to delete). */
export const ARCHIVE_KEYWORD = 'DELETE'

/** The tooltip on every disabled edit of an archived deck. */
export const ARCHIVED_TITLE = 'Deck archived: restore it in the danger zone to make changes'

/** Why a draft name cannot be saved, or null when it can (trimmed, 1 to 255 characters). */
export function deckNameProblem(draft: string): string | null {
  // Spread counts characters (code points), as the app does, not UTF-16 units.
  const length = [...draft.trim()].length
  return length === 0 || length > DECK_NAME_MAX ? NAME_MESSAGE : null
}

/** The trimmed name to save, or null when the draft is invalid or unchanged. */
export function renameTarget(draft: string, current: string): string | null {
  const next = draft.trim()
  if (deckNameProblem(next) !== null || next === current) {
    return null
  }
  return next
}

/** The colour identity toggles, in 0.x order. */
export const IDENTITY_OPTIONS: { key: string; label: string }[] = [
  { key: 'W', label: 'White' },
  { key: 'U', label: 'Blue' },
  { key: 'B', label: 'Black' },
  { key: 'R', label: 'Red' },
  { key: 'G', label: 'Green' },
  { key: 'C', label: 'Colorless' },
]

const IDENTITY_ORDER = IDENTITY_OPTIONS.map((option) => option.key)

/** The letters without repeats, in WUBRGC order. */
export function orderIdentity(colors: string[]): string[] {
  const unique = Array.from(new Set(colors))
  return unique.sort((a, b) => IDENTITY_ORDER.indexOf(a) - IDENTITY_ORDER.indexOf(b))
}

/** The selection with `letter` switched on or off, in WUBRGC order. */
export function toggleIdentity(selected: string[], letter: string): string[] {
  const next = selected.includes(letter) ? selected.filter((color) => color !== letter) : [...selected, letter]
  return orderIdentity(next)
}

/** Whether the selection differs from the saved identity (order aside). */
export function identityChanged(selected: string[], saved: string[]): boolean {
  return orderIdentity(selected).join(',') !== orderIdentity(saved).join(',')
}

/** Whether the typed confirmation allows an archive (trimmed, exactly DELETE). */
export function canArchive(text: string): boolean {
  return text.trim() === ARCHIVE_KEYWORD
}

/**
 * The chosen printing once a card name's printings have loaded (0.x): the
 * only one when there is one, the current choice when the name has it, else
 * none.
 */
export function coverAfterOptions(options: CoverOptionView[], selected: number | null): number | null {
  if (options.length === 1) {
    return options[0].catalogId
  }
  return options.some((option) => option.catalogId === selected) ? selected : null
}

/**
 * The cover picker's query key. Under `['decks']`, so `DecksChanged` and every
 * deck edit refresh it. Kept here rather than in `keys.ts` because it has a
 * single user.
 */
export function coverSettingsKey(deckId: number) {
  return ['decks', 'cover-settings', deckId] as const
}
