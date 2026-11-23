import { describe, expect, it } from 'vitest'
import { ref } from 'vue'
import type { DeckKindsView } from '@/bindings'
import { useDeckBestOf } from './useDeckBestOf'

function fakeStorage(initial: Record<string, string> = {}) {
  const data = { ...initial }
  return {
    data,
    getItem: (key: string) => data[key] ?? null,
    setItem: (key: string, value: string) => {
      data[key] = value
    },
  }
}

const both: DeckKindsView = { bo1: 4, bo3: 6, latest: 'bo1' }

describe('useDeckBestOf', () => {
  it('has no kind, no switch and no Bo1 mode for an MTGO deck', () => {
    const state = useDeckBestOf(ref(1), ref(null), fakeStorage())
    expect(state.bestOf.value).toBeNull()
    expect(state.showSwitch.value).toBe(false)
    expect(state.bo1Mode.value).toBe(false)
  })

  it('uses the only kind with matches, with no switch, and Bo1 mode for bo1', () => {
    const state = useDeckBestOf(ref(1), ref({ bo1: 3, bo3: 0, latest: 'bo1' }), fakeStorage())
    expect(state.bestOf.value).toBe('bo1')
    expect(state.showSwitch.value).toBe(false)
    expect(state.bo1Mode.value).toBe(true)

    const bo3Only = useDeckBestOf(ref(1), ref({ bo1: 0, bo3: 3, latest: 'bo3' }), fakeStorage())
    expect(bo3Only.bestOf.value).toBe('bo3')
    expect(bo3Only.bo1Mode.value).toBe(false)
  })

  it('has no kind when neither has matches', () => {
    const state = useDeckBestOf(ref(1), ref({ bo1: 0, bo3: 0, latest: null }), fakeStorage())
    expect(state.bestOf.value).toBeNull()
    expect(state.showSwitch.value).toBe(false)
  })

  it('starts on the latest kind when both have matches and nothing is stored', () => {
    const state = useDeckBestOf(ref(1), ref(both), fakeStorage())
    expect(state.bestOf.value).toBe('bo1')
    expect(state.showSwitch.value).toBe(true)
  })

  it('honours the stored choice and stores a new one under the deck key', () => {
    const storage = fakeStorage({ 'deck-best-of:7': 'bo3' })
    const state = useDeckBestOf(ref(7), ref(both), storage)
    expect(state.bestOf.value).toBe('bo3')
    state.choose('bo1')
    expect(state.bestOf.value).toBe('bo1')
    expect(storage.data['deck-best-of:7']).toBe('bo1')
  })

  it('falls back when the stored kind has no matches', () => {
    const storage = fakeStorage({ 'deck-best-of:7': 'bo3' })
    const state = useDeckBestOf(ref(7), ref({ bo1: 2, bo3: 0, latest: 'bo1' }), storage)
    expect(state.bestOf.value).toBe('bo1')
  })

  it('gives the remembered kind until the kinds are read', () => {
    const storage = fakeStorage({ 'deck-best-of:7': 'bo3' })
    const kinds = ref<DeckKindsView | null | undefined>(undefined)
    const state = useDeckBestOf(ref(7), kinds, storage)
    expect(state.bestOf.value).toBe('bo3')
    expect(state.showSwitch.value).toBe(false)
    kinds.value = { bo1: 2, bo3: 0, latest: 'bo1' }
    expect(state.bestOf.value).toBe('bo1')
    expect(useDeckBestOf(ref(8), ref(undefined), storage).bestOf.value).toBeNull()
  })

  it('reads the stored choice again for another deck', () => {
    const storage = fakeStorage({ 'deck-best-of:1': 'bo3', 'deck-best-of:2': 'bo1' })
    const deckId = ref(1)
    const state = useDeckBestOf(deckId, ref(both), storage)
    expect(state.bestOf.value).toBe('bo3')
    deckId.value = 2
    expect(state.bestOf.value).toBe('bo1')
  })
})
