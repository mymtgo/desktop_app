import { useDebounceFn } from '@vueuse/core'
import { ref, type Ref } from 'vue'
import { commands } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { unwrap } from '@/lib/commands'
import { clampTrust, TRUST_DEFAULT } from '@/lib/cardStats/format'

const SAVE_DEBOUNCE_MS = 300

export interface UseTrustSetting {
  readonly value: Ref<number>
  /** Take the stored value once; later calls leave the slider alone. */
  seed(stored: number): void
  setValue(next: number): void
  reset(): void
}

/**
 * The "Games to trust" slider: saved 300 ms after the last change. A failed
 * save puts the last saved value back and says so (0.x `useTrustSetting`).
 */
export function useTrustSetting(): UseTrustSetting {
  const value = ref(TRUST_DEFAULT)
  let lastSaved = TRUST_DEFAULT
  let seeded = false
  const { add } = useToast()

  async function save(next: number): Promise<void> {
    try {
      lastSaved = unwrap(await commands.setCardStatsTrust(next))
    } catch {
      value.value = lastSaved
      add({
        type: 'error',
        title: 'Could not save trust preference',
        message: 'Your slider value will reset next time you open the page.',
      })
    }
  }

  const saveSoon = useDebounceFn((next: number) => {
    void save(next)
  }, SAVE_DEBOUNCE_MS)

  function seed(stored: number): void {
    if (seeded) return
    seeded = true
    value.value = clampTrust(stored)
    lastSaved = value.value
  }

  function setValue(next: number): void {
    const clamped = clampTrust(next)
    if (clamped === value.value) return
    value.value = clamped
    saveSoon(clamped)
  }

  function reset(): void {
    setValue(TRUST_DEFAULT)
  }

  return { value, seed, setValue, reset }
}
