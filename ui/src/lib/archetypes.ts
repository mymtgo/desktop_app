import { keepPreviousData, useQuery } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'
import type { LocationQuery } from 'vue-router'
import {
  commands,
  type ArchetypeChoiceView,
  type ArchetypeSourceView,
  type FormCardInput,
  type ListCardView,
  type ManageQueryInput,
  type ResolvedCardView,
  type TypeGroupView,
} from '@/bindings'
import { readStoredJson, writeStoredJson, type KeyValueStorage } from './browserStorage'
import { unwrap } from './commands'
import { MANA_COLOR_NAMES } from './mana'

// Manual archetypes and the manage screen (archetypes spec sections 7 and
// 11, 0.x `pages/archetypes`).

/** The form's formats (0.x `ArchetypeForm.vue` `FORMATS`). */
export const ARCHETYPE_FORMATS: { value: string; label: string }[] = [
  { value: 'standard', label: 'Standard' },
  { value: 'pioneer', label: 'Pioneer' },
  { value: 'modern', label: 'Modern' },
  { value: 'pauper', label: 'Pauper' },
  { value: 'legacy', label: 'Legacy' },
  { value: 'vintage', label: 'Vintage' },
  { value: 'premodern', label: 'Premodern' },
]

/** 0.x `ManaIdentityPicker` `SYMBOLS`: WUBRG then colourless. */
export const COLOR_SYMBOLS: { key: string; label: string }[] = Object.entries(MANA_COLOR_NAMES).map(([key, label]) => ({
  key,
  label,
}))

/** The letters of a comma-form identity ("U,B"). */
export function colorLetters(identity: string | null | undefined): string[] {
  return (identity ?? '').split(',').map((letter) => letter.trim()).filter(Boolean)
}

/** Toggle one colour (0.x `ManaIdentityPicker.toggle`): ordered, `null` when none is left. */
export function toggleColor(identity: string | null, key: string): string | null {
  const current = new Set(colorLetters(identity))
  if (current.has(key)) current.delete(key)
  else current.add(key)
  const ordered = COLOR_SYMBOLS.map((symbol) => symbol.key).filter((letter) => current.has(letter))
  return ordered.length ? ordered.join(',') : null
}

export const NAME_MAX = 255

/** The form's first problem, in the server's words (`check_form`), else `null`. */
export function formProblem(form: { name: string; format: string; cardCount: number; requireCards: boolean }): string | null {
  const name = form.name.trim()
  if (name === '') return 'The name field is required.'
  if ([...name].length > NAME_MAX) return 'The name field must not be greater than 255 characters.'
  if (form.format.trim() === '') return 'The format field is required.'
  if (form.requireCards && form.cardCount === 0) return 'The cards field is required.'
  return null
}

/** 0.x `submitButtonLabel`: an edit with cards loaded adds a variant. */
export function submitButtonLabel(requireCards: boolean, cardCount: number, label: string): string {
  if (requireCards) return label
  return cardCount > 0 ? 'Save & Add Variant' : label
}

/** The resolved cards as the form sends them. */
export function formCards(cards: ResolvedCardView[]): FormCardInput[] {
  return cards.map((card) => ({
    catalogId: card.catalogId,
    oracleId: card.oracleId,
    name: card.name,
    quantity: card.quantity,
    sideboard: card.sideboard,
  }))
}

/**
 * The badge an archetype shows (spec section 10). A server archetype held
 * as Custom by the offline download is the website's, so it shows none.
 */
export function sourceBadge(source: ArchetypeSourceView, retired: boolean, offlineHeld: boolean): string | null {
  if (retired) return 'Retired'
  if (offlineHeld) return null
  if (source === 'manual') return 'Custom'
  if (source === 'fallback') return 'System'
  if (source === 'commander') return 'Commander'
  return null
}

// ---- Routes ---------------------------------------------------------------

export const MANAGE_PATH = '/metagame/manage'

/** The archetype detail. */
export function archetypeDetailPath(id: number): string {
  return `/metagame/archetypes/${id}`
}

export function archetypeEditPath(id: number): string {
  return `/metagame/archetypes/${id}/edit`
}

/**
 * The create form. With `sourceMatchId` the form is prefilled from that
 * match's opponent cards (T8's "Create archetype from this match").
 */
export function createArchetypeRoute(prefill: { sourceMatchId?: number; format?: string } = {}) {
  const query: Record<string, string> = {}
  if (prefill.sourceMatchId !== undefined) query.source_match_id = String(prefill.sourceMatchId)
  if (prefill.format) query.format = prefill.format
  return { path: '/metagame/archetypes/new', query }
}

// ---- The manage query (0.x GetFilteredArchetypes, StickyFiltersTest) ------

/** `manual` (Custom), `downloaded` (Server), `commander` (Commander), or '' for every source. */
export type ManageSource = '' | 'manual' | 'downloaded' | 'commander'

export interface ManageFilters {
  search: string
  format: string
  source: ManageSource
}

function manageSource(value: unknown): ManageSource {
  return value === 'manual' || value === 'downloaded' || value === 'commander' ? value : ''
}

export interface ManageState extends ManageFilters {
  page: number
}

/** Where the last filters are remembered (0.x kept them in the session). */
export const MANAGE_FILTERS_KEY = 'mymtgo.metagame.manage.filters'

function text(value: LocationQuery[string] | undefined): string {
  const first = Array.isArray(value) ? value[0] : value
  return typeof first === 'string' ? first : ''
}

/**
 * The manage state from the URL. A query that names a filter (even empty,
 * which is how one is cleared) is authoritative and is remembered
 * (`remember`); without one, the remembered filters fill the silence.
 */
export function manageFromUrl(
  query: LocationQuery,
  remembered: ManageFilters | null,
): { state: ManageState; remember: ManageFilters | null } {
  const page = Math.max(1, Number.parseInt(text(query.page), 10) || 1)
  if ('search' in query || 'format' in query || 'source' in query) {
    const filters = {
      search: text(query.search).trim(),
      format: text(query.format),
      source: manageSource(text(query.source)),
    }
    return { state: { ...filters, page }, remember: filters }
  }
  const filters = remembered ?? { search: '', format: '', source: '' }
  return { state: { ...filters, page }, remember: null }
}

/** The URL query of a manage state (every filter always named). */
export function manageUrlQuery(state: ManageState): Record<string, string> {
  const query: Record<string, string> = { search: state.search, format: state.format, source: state.source }
  if (state.page > 1) query.page = String(state.page)
  return query
}

export function readRememberedFilters(storage: KeyValueStorage): ManageFilters | null {
  // Junk in storage: nothing remembered.
  const parsed = readStoredJson(storage, MANAGE_FILTERS_KEY)
  if (parsed && typeof parsed === 'object') {
    const { search, format, source } = parsed as Record<string, unknown>
    if (typeof search === 'string' && typeof format === 'string') {
      return { search, format, source: manageSource(source) }
    }
  }
  return null
}

export function writeRememberedFilters(storage: KeyValueStorage, filters: ManageFilters) {
  writeStoredJson(storage, MANAGE_FILTERS_KEY, filters)
}

// ---- Decklists --------------------------------------------------------------

/** One list's cards as `DeckList` takes them: one group per card (`groupedMaindeck` merges them). */
export function groupListCards(cards: ListCardView[]): { maindeck: TypeGroupView[]; sideboard: ListCardView[] } {
  const maindeck: TypeGroupView[] = []
  const sideboard: ListCardView[] = []
  for (const card of cards) {
    if (card.sideboard) sideboard.push(card)
    else maindeck.push({ typeName: card.typeLine ?? 'Unknown', count: card.quantity, cards: [card] })
  }
  return { maindeck, sideboard }
}

// ---- Queries ------------------------------------------------------------------

export const archetypeKeys = {
  all: ['archetypes'] as const,
  manage: (query: ManageQueryInput) => ['archetypes', 'manage', query] as const,
  form: (id: number) => ['archetypes', 'form', id] as const,
  /** The create form filled from a match (`source_match_id`). */
  prefill: (matchId: number) => ['archetypes', 'prefill', matchId] as const,
  formMatches: (format: string, search: string) => ['archetypes', 'form-matches', format, search] as const,
  candidates: (kind: CandidateKind, id: number) => ['archetypes', 'candidates', kind, id] as const,
}

export function useManageArchetypes(query: Ref<ManageQueryInput>) {
  return useQuery({
    queryKey: computed(() => archetypeKeys.manage(query.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.manageArchetypes(queryKey[2])),
    placeholderData: keepPreviousData,
  })
}

export function useArchetypeForm(id: Ref<number | null>) {
  return useQuery({
    queryKey: computed(() => archetypeKeys.form(id.value ?? 0)),
    queryFn: async ({ queryKey }) => unwrap(await commands.archetypeFormData(queryKey[2])),
    enabled: computed(() => id.value !== null),
  })
}

/** Which dialog lists candidates: merge (any non-fallback) or reassign (manual only). */
export type CandidateKind = 'merge' | 'reassign'

/**
 * The candidate list of a dialog (T7 review I1): "Move to archetype" offers
 * only the targets `reassign_variant` accepts, "Merge" every merge candidate.
 */
export function candidateLoader(kind: CandidateKind): (id: number) => Promise<ArchetypeChoiceView[]> {
  if (kind === 'reassign') return async (id) => unwrap(await commands.reassignCandidates(id))
  return async (id) => unwrap(await commands.mergeCandidates(id))
}

/** The candidates whose name holds the search, any case. */
export function filterCandidates(candidates: ArchetypeChoiceView[], search: string): ArchetypeChoiceView[] {
  const term = search.trim().toLowerCase()
  return term === '' ? candidates : candidates.filter((candidate) => candidate.name.toLowerCase().includes(term))
}

/**
 * The detail page's merged notice, exactly as 0.x
 * (`ArchetypeDetail.vue`): the parent's name sits between the two parts
 * (it is a link on the page).
 */
export function mergedNotice(parentName: string): { before: string; name: string; after: string } {
  return {
    before: 'This archetype has been merged into ',
    name: parentName,
    after: '. Future detections will be attributed to it.',
  }
}
