import { useQueryClient } from '@tanstack/vue-query'
import { ref } from 'vue'
import { commands, events } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'

/** The match pages' queries (`matchDetailKeys.all`). */
const MATCH_DETAIL = ['matchDetail'] as const

/** `n match` or `n matches`. */
export function matchCount(count: number): string {
  return `${count} match${count === 1 ? '' : 'es'}`
}

/**
 * Archetype detection asked for by the player (archetypes spec section
 * 6.3): "Detect" on match rows and on the selected matches, detected now on
 * the archetypes worker (several matches end with a count toast, none
 * detected with 0.x's "Detection failed" toast), and
 * the deck "Detect archetypes" banner, which queues the deck's matches
 * under the archetype filter (0.x `QueueArchetypeDetectionForDeck`).
 * Lists refetch afterwards; failures become toasts.
 */
export function useArchetypeDetection() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const detecting = ref(false)
  // The matches "Detect" is running on, for the tables' spinners. Each one
  // leaves as the worker writes it (`matchDetected`), once its row is
  // refetched, so the answers show one by one.
  const detectingTokens = ref<string[]>([])

  async function onDetected(token: string) {
    if (!detectingTokens.value.includes(token)) return
    await refresh()
    detectingTokens.value = detectingTokens.value.filter((shown) => shown !== token)
  }

  async function refresh() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: keys.allMatches }),
      queryClient.invalidateQueries({ queryKey: keys.allDecks }),
      queryClient.invalidateQueries({ queryKey: MATCH_DETAIL }),
    ])
  }

  /** "Detect" on matches by token. */
  async function detectMatches(tokens: string[]): Promise<void> {
    if (tokens.length === 0 || detecting.value) return
    detecting.value = true
    detectingTokens.value = [...tokens]
    const stopListening = await events.matchDetected.listen((event) => void onDetected(event.payload.token))
    try {
      const detected = unwrap(await commands.detectArchetypes(tokens))
      if (detected === 0) {
        // Nothing written (0.x's toast when the match was left without one):
        // a failed request, or a match with nothing to detect from.
        const message =
          tokens.length === 1
            ? "Could not determine the opponent's archetype for this match."
            : "Could not determine the opponents' archetypes for these matches."
        add({ type: 'error', title: 'Detection failed', message })
      } else if (tokens.length > 1) {
        add({ type: 'success', title: 'Archetypes detected', message: `Detected archetypes for ${matchCount(detected)}` })
      }
      await refresh()
    } catch (error) {
      add({ type: 'error', title: 'Detection failed', message: errorMessage(error) })
    } finally {
      stopListening()
      detecting.value = false
      detectingTokens.value = []
    }
  }

  /** The deck banner: queue the deck's matches under `filter` (`none` or an archetype id). */
  async function queueDeck(deckId: number, filter: string): Promise<void> {
    if (detecting.value) return
    detecting.value = true
    try {
      const queued = unwrap(await commands.detectDeckArchetypes(deckId, filter))
      add({
        type: 'success',
        title: 'Detection queued',
        message: `Queued archetype detection for ${matchCount(queued)}`,
      })
      await refresh()
    } catch {
      add({ type: 'error', title: 'Failed', message: 'Could not queue archetype detection.' })
    } finally {
      detecting.value = false
    }
  }

  return { detecting, detectingTokens, detectMatches, queueDeck }
}
