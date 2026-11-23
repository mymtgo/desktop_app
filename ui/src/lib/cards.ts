import { convertFileSrc } from '@tauri-apps/api/core'
import { computed, ref, watch } from 'vue'
import type { CardDataStatus, CardImagesStatus } from '@/bindings'
import { FILTER_KEYS, type TypeFilters } from './cardStats/typeFilter'
import { CANONICAL_TYPES } from './cardTypes'

/** The app's URI scheme for v1's local card images (`app/src/cards/scheme.rs`). */
export const CARD_SCHEME = 'card'

/** The app's URI scheme for a deck's own cover image (`app/src/decks/cover.rs`). */
export const DECK_COVER_SCHEME = 'deckcover'

/**
 * The Settings "Card data" card's summary (0.x `CardDataCard`). A device that
 * took its history from cloud sync starts with no cards at all, so "none yet"
 * is the state the card exists for.
 */
export function cardDataSummary(status: CardDataStatus): string {
  if (status.total === 0) {
    return 'No card details downloaded yet. Decks and matches will show without names or artwork until you download them.'
  }
  if (status.incomplete > 0) {
    return `${status.total} cards stored, ${status.incomplete} still missing details.`
  }
  return `${status.total} cards stored, all with details.`
}

/** The card's button text while a download runs, and otherwise. */
export function downloadCardDataLabel(running: boolean): string {
  return running ? 'Downloading...' : 'Download card data'
}

/**
 * Where a card image loads from: the downloaded file through the `card`
 * scheme when there is one, else the remote URL. For CSS backgrounds; an
 * `<img>` uses `CardImage`, which also falls back when the file is gone.
 */
export function cardImageSrc(remote: string | null, local: string | null): string | null {
  return local ? convertFileSrc(local, CARD_SCHEME) : remote
}

/**
 * `CardImage`'s source: the deck's own cover image when there is one, then
 * the local file, then the remote URL. Each step is dropped once, on a load
 * error (a missing drive or a deleted cover shows the next one, never a
 * loop). A new file is tried again.
 */
export function useCardImage(props: { src: string | null; local?: string | null; custom?: string | null }) {
  const failed = ref(false)
  const customFailed = ref(false)
  watch(
    () => props.local,
    () => {
      failed.value = false
    },
  )
  watch(
    () => props.custom,
    () => {
      customFailed.value = false
    },
  )
  const current = computed(() => {
    if (props.custom && !customFailed.value) return convertFileSrc(props.custom, DECK_COVER_SCHEME)
    return failed.value ? props.src : cardImageSrc(props.src, props.local ?? null)
  })
  function onError() {
    if (props.custom && !customFailed.value) {
      customFailed.value = true
    } else if (props.local) {
      failed.value = true
    }
  }
  return { current, onError }
}

/** The hover preview's open and close delays (0.x `CardHoverPreview`). */
export const PREVIEW_OPEN_DELAY_MS = 150
export const PREVIEW_CLOSE_DELAY_MS = 50

/** Whether a card has an image to preview on hover; none shows the content bare. */
export function hasCardPreview(src: string | null | undefined, local: string | null | undefined): boolean {
  return Boolean(src) || Boolean(local)
}

/** The red notice when a chosen images folder is gone (0.x `LocalImagesCard`). */
export const MISSING_FOLDER_TEXT =
  'Image folder not found. Downloaded images will not show until it is back, and new downloads are paused.'

/** The folder button's text while a move runs, and otherwise. */
export function folderButtonLabel(moving: boolean): string {
  return moving ? 'Moving...' : 'Change'
}

/** How often the "Card images" card refetches: every second while a move runs, else every 5 s (downloads are always running). */
export function cardImagesPollInterval(status: CardImagesStatus | undefined): number {
  if (status?.folder.moving) return 1000
  // Downloads add files all the time, so the usage line keeps up.
  return 5000
}

/** The type filter's categories in canonical order (`store`'s `TYPE_CATEGORIES`); `Other` is never hidden. */
export const CARD_TYPE_KEYS = CANONICAL_TYPES

/** What the Cards page shows; it lives in the URL so back and forward step through it. */
export type CardsState = {
  search: string
  /** A raw format key, or null for every format. */
  format: string | null
  /** Hidden categories, in canonical order. */
  hiddenTypes: string[]
  groupPrintings: boolean
  page: number
}

export const DEFAULT_CARDS: CardsState = { search: '', format: null, hiddenTypes: [], groupPrintings: true, page: 1 }

function first(value: unknown): string {
  const one = Array.isArray(value) ? value[0] : value
  return typeof one === 'string' ? one : ''
}

/** The state a query string holds (0.x `search`, `format`, `hidden_types`, `group_printings`, `page`). */
export function cardsFromUrl(query: Record<string, unknown>): CardsState {
  const named = first(query.hidden_types).split(',')
  const page = Number.parseInt(first(query.page), 10)
  return {
    search: first(query.search).trim(),
    format: first(query.format) || null,
    hiddenTypes: CARD_TYPE_KEYS.filter((key) => named.includes(key)),
    groupPrintings: first(query.group_printings) !== 'false',
    page: Number.isFinite(page) && page >= 1 ? page : 1,
  }
}

/** The query string for a state: defaults are left out, as 0.x does. */
export function cardsUrlQuery(state: CardsState): Record<string, string> {
  const query: Record<string, string> = {}
  if (state.search.trim() !== '') query.search = state.search
  if (state.format) query.format = state.format
  if (state.hiddenTypes.length > 0) query.hidden_types = state.hiddenTypes.join(',')
  if (!state.groupPrintings) query.group_printings = 'false'
  if (state.page > 1) query.page = String(state.page)
  return query
}

/** The popularity badge's tooltip. */
export function popularityTitle(count: number): string {
  return `Used in ${count} archetype${count === 1 ? '' : 's'}`
}

/** The sheet's heading over its rows. */
export function archetypeCountLabel(count: number): string {
  return `Seen in ${count} archetype${count === 1 ? '' : 's'}`
}

/** "Fetch missing" success toast (0.x). */
export const POPULATED_TOAST = { title: 'Populated', message: 'Missing card data fetched.' }

/** The type filter's toggles for the hidden categories (every one on, the Sideboard toggle included, which this page never uses). */
export function typeToggles(hidden: string[]): TypeFilters {
  const toggles = {} as TypeFilters
  for (const key of FILTER_KEYS) toggles[key] = !hidden.includes(key)
  return toggles
}

/** The hidden categories a type filter's toggles hold, in canonical order. */
export function hiddenTypesFrom(toggles: Partial<TypeFilters>): string[] {
  return CARD_TYPE_KEYS.filter((key) => toggles[key] === false)
}
