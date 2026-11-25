import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'
import { commands, type WrappedBundle } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'

/**
 * Wrapped for the page, the nav link and the dashboard card: the year shown
 * now, whether the dashboard card was dismissed, and the bundle.
 */
export function useWrapped(): {
  year: Ref<number | null>
  dismissed: Ref<boolean>
  bundle: Ref<WrappedBundle | null>
  dismiss: () => Promise<void>
} {
  const client = useQueryClient()
  const toast = useToast()
  const { data } = useQuery({
    queryKey: keys.wrappedState,
    queryFn: async () => unwrap(await commands.wrappedState()),
    // Building the bundle holds the database reader for every deck's reads,
    // and the counted year ends on 1 December, so it barely changes while
    // Wrapped shows. An hour still picks up 1 December in a running app.
    staleTime: 60 * 60_000,
  })

  const bundle = computed(() => data.value?.bundle ?? null)
  const year = computed(() => data.value?.year ?? null)
  const dismissed = computed(() => data.value?.dismissed ?? false)

  async function dismiss() {
    if (year.value === null) return
    try {
      const state = unwrap(await commands.dismissWrapped())
      client.setQueryData(keys.wrappedState, state)
    } catch (error) {
      // The card stays up and the player can try again.
      toast.add({ type: 'error', title: 'Could not hide Wrapped', message: errorMessage(error) })
    }
  }

  return { year, dismissed, bundle, dismiss }
}
