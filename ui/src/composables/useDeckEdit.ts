import { useQueryClient } from '@tanstack/vue-query'
import { ref } from 'vue'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap, type CommandResult } from '@/lib/commands'
import { keys } from '@/lib/keys'


/**
 * Runs one deck edit (rename, identity, cover, archive, restore). On success
 * every deck query is refetched straight away (the `DecksChanged` event
 * follows too); a failure or refusal becomes a toast titled `title`.
 * `saving` is true until the refetch is done, so the page never shows the
 * old value after the spinner stops.
 */
export function useDeckEdit() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const saving = ref(false)

  async function run(title: string, action: () => Promise<CommandResult<null>>): Promise<boolean> {
    saving.value = true
    try {
      unwrap(await action())
      await queryClient.invalidateQueries({ queryKey: keys.allDecks })
      return true
    } catch (error) {
      add({ type: 'error', title, message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  return { saving, run }
}
