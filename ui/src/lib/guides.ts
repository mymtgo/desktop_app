import { useQuery } from '@tanstack/vue-query'
import { computed, reactive, ref, type Ref } from 'vue'
import {
  commands,
  type EditorRowView,
  type GuideCardInputView,
  type GuideColumnsView,
  type GuideDeckOptionView,
  type GuideSummaryView,
} from '@/bindings'
import { normalizeType, TYPE_ORDER } from './cardTypes'
import { unwrap, type CommandResult } from './commands'

// Sideboard guides and matchup notes (archetypes spec section 21.4, survey
// section 13.4): the deck tab's list and editor and the archetype page's
// block. The numbers come from the backend; this file holds 0.x's strings,
// the balance chip, the steppers and the editor's autosave.

export type Direction = 'in' | 'out'

/** An empty cell (0.x's em dash, rewritten). */
export const EMPTY_CELL = '-'

/** The plan column (0.x `planSummary`). */
export function planSummary(cardsIn: number, cardsOut: number): string {
  if (cardsIn === 0 && cardsOut === 0) return 'No cards planned'
  return `+${cardsIn} in / -${cardsOut} out`
}

/** `67%`, or the empty cell when never faced. */
export function percentCell(value: number | null): string {
  return value === null ? EMPTY_CELL : `${value}%`
}

/** The Matches column: the record, or the empty cell when never faced. */
export function recordCell(matches: number, record: string | null): string {
  return matches > 0 && record !== null ? record : EMPTY_CELL
}

/** The editor header's line (0.x `SideboardGuideEdit` `record`). */
export function recordLine(guide: Pick<GuideSummaryView, 'matches' | 'matchRecord' | 'matchWinrate' | 'gameWinrate'>): string {
  if (guide.matches === 0) return 'Not faced yet'
  return `${guide.matchRecord} · ${guide.matchWinrate}% match · ${guide.gameWinrate}% game`
}

/** The delete confirm's description. */
export function deleteGuideText(archetypeName: string): string {
  return `Deleting removes the plan and all notes for ${archetypeName}. This cannot be undone.`
}

/** The balance chip (0.x `SideboardGuideBalance`): the board after the plan and what is left to balance it. */
export function balance(totalIn: number, totalOut: number, sideboardSize: number): { resulting: number; hint: string | null } {
  // Cards cut from the maindeck go back to the board, cards brought in leave it.
  const resulting = sideboardSize - totalIn + totalOut
  const shortfall = sideboardSize - resulting
  if (shortfall > 0) return { resulting, hint: `cut ${shortfall} more` }
  if (shortfall < 0) return { resulting, hint: `bring in ${-shortfall} more` }
  return { resulting, hint: null }
}

/** "Your W–L": `{w}–{l} ({rate}%)`, the empty cell without sided-in games. */
export function sidedInText(row: Pick<EditorRowView, 'sidedInGames' | 'wins' | 'losses' | 'winrate'>): string {
  if (row.sidedInGames === 0) return EMPTY_CELL
  return `${row.wins}–${row.losses} (${row.winrate ?? 0}%)`
}

/** "You cut": `{n}×`, the empty cell when never cut. */
export function youCutText(row: Pick<EditorRowView, 'sidedOutGames'>): string {
  return row.sidedOutGames > 0 ? `${row.sidedOutGames}×` : EMPTY_CELL
}

/** The editor's footer line. */
export function postboardText(games: number, record: string): string {
  if (games > 0) return `${games} post-board games · ${record} overall`
  return 'No post-board games vs this archetype yet'
}

/** The stale badge (0.x `SideboardGuideCardRow`). */
export function staleLabel(direction: Direction): string {
  return direction === 'in' ? 'Not in sideboard' : 'Not in maindeck'
}

/**
 * A stepper click (0.x `SideboardGuideCardRow.step`): 0 up to the zone's
 * copies; a stale row (the card left that zone) can only go down.
 */
export function stepPlanned(planned: number, owned: number, stale: boolean, delta: number): number {
  return Math.max(0, Math.min(stale ? planned : owned, planned + delta))
}

/** Whether "More" is enabled. */
export function canStepUp(planned: number, owned: number, stale: boolean): boolean {
  return !stale && planned < owned
}

/**
 * An editor column grouped by card type (0.x `groupByType`): canonical types
 * in display order, a row without a type as Other, last.
 */
export function groupByType<T extends { typeLine: string | null }>(rows: T[]): { type: string; rows: T[] }[] {
  const groups = new Map<string, T[]>()
  for (const row of rows) {
    const type = row.typeLine ? normalizeType(row.typeLine) : 'Other'
    const group = groups.get(type)
    if (group) group.push(row)
    else groups.set(type, [row])
  }
  return [...groups.entries()]
    .sort(([a], [b]) => (TYPE_ORDER[a] ?? 99) - (TYPE_ORDER[b] ?? 99))
    .map(([type, grouped]) => ({ type, rows: grouped }))
}

// ---- The plan ------------------------------------------------------------------

/** Planned copies keyed `in:<oracleId>` / `out:<oracleId>`. */
export type Selection = Record<string, number>

export function selectionKey(direction: Direction, oracleId: string): string {
  return `${direction}:${oracleId}`
}

/** The saved plan as a selection: every row the plan lists. */
export function seedSelection(columns: GuideColumnsView | null): Selection {
  const selection: Selection = {}
  for (const row of columns?.bringIn ?? []) if (row.planned) selection[selectionKey('in', row.oracleId)] = row.planned
  for (const row of columns?.takeOut ?? []) if (row.planned) selection[selectionKey('out', row.oracleId)] = row.planned
  return selection
}

/** The whole plan to send, by direction then oracle id. */
export function buildPlan(selection: Selection): GuideCardInputView[] {
  return Object.entries(selection)
    .filter(([, quantity]) => quantity > 0)
    .map(([key, quantity]) => {
      const at = key.indexOf(':')
      return { oracleId: key.slice(at + 1), direction: key.slice(0, at), quantity }
    })
    .sort((a, b) => a.direction.localeCompare(b.direction) || a.oracleId.localeCompare(b.oracleId))
}

/** Planned copies per direction. */
export function selectionTotals(selection: Selection): { totalIn: number; totalOut: number } {
  let totalIn = 0
  let totalOut = 0
  for (const [key, quantity] of Object.entries(selection)) {
    if (key.startsWith('in:')) totalIn += quantity
    else totalOut += quantity
  }
  return { totalIn, totalOut }
}

export type SaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'error'

/** The editor's save label (0.x `saveLabel`). */
export function saveLabel(state: SaveState): string | null {
  switch (state) {
    case 'pending':
    case 'saving':
      return 'Saving…'
    case 'saved':
      return 'Saved'
    case 'error':
      return 'Not saved'
    default:
      return null
  }
}

/** The editor's autosave delay (0.x `SAVE_DELAY_MS`). */
export const SAVE_DELAY_MS = 500

/** 0.x's message when a save fails without a field message. */
export const SAVE_FAILED = 'Could not save your changes.'

/**
 * The editor's autosave (0.x `SideboardGuideEdit`): each change restarts a
 * 500 ms debounce, then the whole plan is sent. A change made while a save
 * runs saves again once it finishes. The selection stays local truth; only
 * a refused save reverts it to the plan last saved, showing the first
 * message; a failed call keeps it and shows 0.x's message. `save` answers the first message of a refusal, or null once
 * saved.
 */
export function useGuideAutosave(save: (cards: GuideCardInputView[]) => Promise<string | null>, delayMs = SAVE_DELAY_MS) {
  const selection = reactive<Selection>({})
  const state = ref<SaveState>('idle')
  const error = ref<string | null>(null)
  let saved: Selection = {}
  let lastSent = JSON.stringify([])
  let timer: ReturnType<typeof setTimeout> | null = null
  let inFlight = false

  function replace(next: Selection) {
    for (const key of Object.keys(selection)) delete selection[key]
    Object.assign(selection, next)
  }

  /** Start from the saved plan (a newly opened guide). */
  function reset(plan: Selection) {
    if (timer) clearTimeout(timer)
    timer = null
    saved = { ...plan }
    replace(plan)
    lastSent = JSON.stringify(buildPlan(plan))
    state.value = 'idle'
    error.value = null
  }

  function schedule() {
    if (timer) clearTimeout(timer)
    state.value = 'pending'
    timer = setTimeout(flush, delayMs)
  }

  function flush() {
    if (timer) clearTimeout(timer)
    timer = null
    if (inFlight) return
    const cards = buildPlan(selection)
    const payload = JSON.stringify(cards)
    if (payload === lastSent) {
      if (state.value === 'pending') state.value = 'saved'
      return
    }
    const sent: Selection = { ...selection }
    let failed = false
    inFlight = true
    state.value = 'saving'
    error.value = null
    save(cards)
      .catch(() => SAVE_FAILED)
      .then((message) => {
        if (message === null) {
          saved = sent
          lastSent = payload
          state.value = 'saved'
        } else if (message === SAVE_FAILED) {
          // The call failed (0.x: a server error): the plan stays as edited
          // and the next change sends it again.
          failed = true
          error.value = message
          state.value = 'error'
        } else {
          error.value = message
          state.value = 'error'
          replace({ ...saved })
          lastSent = JSON.stringify(buildPlan(saved))
        }
      })
      .finally(() => {
        inFlight = false
        // A change made meanwhile is sent now, even after a failed save
        // (#300 F14); the plan that just failed is not sent again on its own.
        const current = JSON.stringify(buildPlan(selection))
        if (current !== lastSent && (!failed || current !== payload)) flush()
      })
  }

  /** A stepper's new value for one card. */
  function set(direction: Direction, oracleId: string, value: number) {
    const key = selectionKey(direction, oracleId)
    if (value === 0) delete selection[key]
    else selection[key] = value
    schedule()
  }

  /** Leaving the page sends a debounced change straight away. */
  function leave() {
    if (timer) flush()
  }

  return { selection, state, error, label: computed(() => saveLabel(state.value)), reset, set, flush, leave }
}

// ---- The create dialog -------------------------------------------------------------

/**
 * The create dialog (0.x `CreateSideboardGuideDialog`): choosing an
 * archetype creates the guide; a refusal (the pair has one) is shown
 * inline and the dialog stays open, so another archetype can be chosen.
 */
export function useCreateGuide(create: (archetypeId: number) => Promise<CommandResult<number>>) {
  const open = ref(false)
  const search = ref('')
  const error = ref<string | null>(null)
  const processing = ref(false)

  function show() {
    search.value = ''
    error.value = null
    open.value = true
  }

  /** The new guide's id, or null when refused. */
  async function select(archetypeId: number): Promise<number | null> {
    if (processing.value) return null
    processing.value = true
    error.value = null
    try {
      const result = await create(archetypeId)
      if (result.status === 'ok') {
        open.value = false
        return result.data
      }
      error.value = result.error.message
      return null
    } finally {
      processing.value = false
    }
  }

  return { open, search, error, processing, show, select }
}

// ---- The archetype page's block ------------------------------------------------

/** The deck select's default: the deck most played against the archetype (the options come in that order). */
export function defaultDeck(options: GuideDeckOptionView[]): number | null {
  return options[0]?.deckId ?? null
}

/** The block's line when no deck has a guide or notes. */
export const NO_GUIDES_TEXT = 'No sideboard guides or notes against this archetype yet.'

/** The line when the player has no deck of the archetype's format. */
export const NO_DECKS_TEXT = 'No deck of this format to write a guide for.'

/** The guide editor's route. */
export function guidePath(deckId: number, guideId: number): string {
  return `/decks/${deckId}/sideboard-guides/${guideId}`
}

/** The deck's guide list route. */
export function guidesPath(deckId: number): string {
  return `/decks/${deckId}/sideboard-guides`
}

// ---- Queries ---------------------------------------------------------------------

/** Under `['decks']`, so `DecksChanged` refetches them; `GuidesChanged` refetches `all`. */
export const guideKeys = {
  all: ['decks', 'guides'] as const,
  list: (deckId: number) => ['decks', 'guides', 'list', deckId] as const,
  editor: (deckId: number, guideId: number) => ['decks', 'guides', 'editor', deckId, guideId] as const,
  archetype: (archetypeId: number) => ['decks', 'guides', 'archetype', archetypeId] as const,
  deckOptions: (archetypeId: number) => ['decks', 'guides', 'deckOptions', archetypeId] as const,
  /** The create dialog's archetypes for the deck's format, asked after first paint. */
  options: (format: string) => ['decks', 'guides', 'options', format] as const,
}

export function useDeckGuides(deckId: Ref<number>) {
  return useQuery({
    queryKey: computed(() => guideKeys.list(deckId.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckGuides(queryKey[3])),
  })
}

export function useGuideEditor(deckId: Ref<number>, guideId: Ref<number>) {
  return useQuery({
    queryKey: computed(() => guideKeys.editor(deckId.value, guideId.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.guideEditor(queryKey[3], queryKey[4])),
  })
}

export function useArchetypeGuides(archetypeId: Ref<number>) {
  return useQuery({
    queryKey: computed(() => guideKeys.archetype(archetypeId.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.archetypeGuides(queryKey[3])),
  })
}

export function useGuideDeckOptions(archetypeId: Ref<number>) {
  return useQuery({
    queryKey: computed(() => guideKeys.deckOptions(archetypeId.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.guideDeckOptions(queryKey[3])),
  })
}

/** The create dialog's archetypes, once `enabled` (the list painted). */
export function useGuideArchetypeOptions(format: Ref<string | null>, enabled: Ref<boolean>) {
  return useQuery({
    queryKey: computed(() => guideKeys.options(format.value ?? '')),
    queryFn: async ({ queryKey }) => unwrap(await commands.archetypeOptions(queryKey[3] || null)),
    enabled,
  })
}
