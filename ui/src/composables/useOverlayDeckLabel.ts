import { useDebounceFn } from '@vueuse/core'
import { ref, watch, type Ref } from 'vue'
import { commands } from '@/bindings'

/** How long after the last keystroke a label is saved (0.x). */
export const LABEL_SAVE_DELAY_MS = 500
/** The most characters stored (spec 9.2). */
const MAX_LABEL_CHARS = 40

/**
 * A streamer-chosen name for the deck on the league card (overlay spec 9.2;
 * 0.x `useOverlayDeckLabel.ts`), stored as the deck's `overlay_label`
 * through `set_overlay_deck_label` and starting from the pushed card's
 * label. Pushes never overwrite typing: they are ignored while a keystroke
 * is unsaved, and after a save until the card echoes that save back, so an
 * older push landing late cannot flicker the old label in. 0.x's
 * `localStorage` migration is not needed.
 */
export function useOverlayDeckLabel(deckId: Ref<number | null>, initial: Ref<string | null>) {
  const label = ref('')
  /** Keystrokes so far, and how many of them the last finished save covered. */
  let edits = 0
  let savedEdits = 0
  /** The label the card should echo back after our save; pushes that differ are older. */
  let awaiting: string | null = null

  const persist = useDebounceFn(async (id: number, value: string) => {
    const covering = edits
    if (await save(id, value)) awaiting = normalise(value)
    savedEdits = Math.max(savedEdits, covering)
  }, LABEL_SAVE_DELAY_MS)

  watch(
    deckId,
    (id) => {
      edits = 0
      savedEdits = 0
      awaiting = null
      label.value = id === null ? '' : (initial.value ?? '')
    },
    { immediate: true },
  )

  watch(initial, (value) => {
    if (deckId.value === null || edits !== savedEdits) return
    if (awaiting !== null) {
      if ((value ?? '') !== awaiting) return
      awaiting = null
    }
    label.value = value ?? ''
  })

  function setLabel(value: string): void {
    label.value = value
    if (deckId.value === null) return
    edits++
    void persist(deckId.value, value)
  }

  return { label, setLabel }
}

/** What is stored for a typed value: trimmed and cut to 40 characters. */
function normalise(value: string): string {
  return [...value.trim()].slice(0, MAX_LABEL_CHARS).join('')
}

async function save(deckId: number, value: string): Promise<boolean> {
  try {
    const result = await commands.setOverlayDeckLabel(deckId, value.trim() === '' ? null : value)
    return result.status === 'ok'
  } catch {
    return false
  }
}
