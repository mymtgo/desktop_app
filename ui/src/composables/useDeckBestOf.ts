import { computed, inject, provide, ref, type ComputedRef, type InjectionKey, type Ref } from 'vue'
import type { BestOfView, DeckKindsView } from '@/bindings'
import { readStored, writeStored, type KeyValueStorage } from '@/lib/browserStorage'

export type DeckBestOf = {
  /** The kind the deck pages read: null for an MTGO deck (or an Arena deck with no matches). */
  bestOf: ComputedRef<BestOfView | null>
  /** Both kinds have matches: the sidebar offers the switch. */
  showSwitch: ComputedRef<boolean>
  /** Bo1 on screen: Game Stats and Sideboard Guides have no data. */
  bo1Mode: ComputedRef<boolean>
  choose: (kind: BestOfView) => void
}

const storageKey = (deckId: number) => `deck-best-of:${deckId}`

function browserStorage(): KeyValueStorage | null {
  try {
    return typeof window === 'undefined' ? null : window.localStorage
  } catch {
    return null
  }
}

/** The kind a deck remembered, whatever else is true now; null when none. */
export function storedBestOf(deckId: number, storage: KeyValueStorage | null = browserStorage()): BestOfView | null {
  const value = storage === null ? null : readStored(storage, storageKey(deckId))
  return value === 'bo1' || value === 'bo3' ? value : null
}

/**
 * Which game structure an Arena deck's pages show. One kind with matches:
 * that one. Both: the remembered choice (kept per deck in `localStorage`),
 * else the kind played last. A remembered kind with no matches is ignored.
 * Until the deck's kinds are read (`undefined`), the remembered kind, so the
 * first reads ask for it.
 */
export function useDeckBestOf(
  deckId: Ref<number>,
  kinds: Ref<DeckKindsView | null | undefined>,
  storage: KeyValueStorage | null = browserStorage(),
): DeckBestOf {
  // The choice made in this window; undefined until one is, then the stored one is read.
  const chosen = ref<{ deckId: number; kind: BestOfView } | null>(null)

  const remembered = computed<BestOfView | null>(() => {
    if (chosen.value !== null && chosen.value.deckId === deckId.value) return chosen.value.kind
    return storedBestOf(deckId.value, storage)
  })

  const showSwitch = computed(() => !!kinds.value && kinds.value.bo1 > 0 && kinds.value.bo3 > 0)

  const bestOf = computed<BestOfView | null>(() => {
    const k = kinds.value
    if (k === undefined) return remembered.value
    if (k === null) return null
    if (k.bo1 > 0 && k.bo3 === 0) return 'bo1'
    if (k.bo3 > 0 && k.bo1 === 0) return 'bo3'
    if (k.bo1 === 0) return null
    const pick = remembered.value
    return pick ?? k.latest ?? 'bo3'
  })

  function choose(kind: BestOfView) {
    chosen.value = { deckId: deckId.value, kind }
    if (storage !== null) writeStored(storage, storageKey(deckId.value), kind)
  }

  return { bestOf, showSwitch, bo1Mode: computed(() => bestOf.value === 'bo1'), choose }
}

const DECK_BEST_OF: InjectionKey<DeckBestOf> = Symbol('deck-best-of')

/** The deck layout shares its one instance, so every page and the sidebar agree. */
export function provideDeckBestOf(state: DeckBestOf) {
  provide(DECK_BEST_OF, state)
}

const NO_KIND: DeckBestOf = {
  bestOf: computed(() => null),
  showSwitch: computed(() => false),
  bo1Mode: computed(() => false),
  choose: () => {},
}

/** The layout's instance; outside a deck layout, no kind (MTGO behaviour). */
export function useDeckBestOfContext(): DeckBestOf {
  return inject(DECK_BEST_OF, NO_KIND)
}
