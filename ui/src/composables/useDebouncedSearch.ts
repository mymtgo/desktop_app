import { refDebounced } from '@vueuse/core'
import { computed, ref, watch, type Ref } from 'vue'

/** The search box wait before a typed search is applied. */
export const SEARCH_DEBOUNCE_MS = 300

export interface DebouncedSearchOptions {
  /** The applied search (usually read from the URL). */
  current: () => string
  /** Applies a settled search that differs from `current`. */
  apply: (next: string) => void
  /** Wait after the last key, 300 ms unless given. */
  delayMs?: number
  /** Turns the typed text into the applied value, trimming unless given. */
  normalise?: (value: string) => string
}

/**
 * A search box bound to an applied search: typing is applied once it settles,
 * and a change from outside (back and forward) is followed, except while
 * typing is still waiting to be applied (typing wins).
 */
export function useDebouncedSearch(options: DebouncedSearchOptions): Ref<string> {
  const normalise = options.normalise ?? ((value: string) => value.trim())
  const search = ref(options.current())
  const settled = refDebounced(search, options.delayMs ?? SEARCH_DEBOUNCE_MS)
  const waiting = computed(() => search.value !== settled.value)

  watch(settled, (next) => {
    const value = normalise(next)
    if (value !== options.current()) options.apply(value)
  })

  watch(options.current, (next) => {
    if (!waiting.value && next !== normalise(search.value)) search.value = next
  })

  return search
}
