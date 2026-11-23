import { useQueryClient, type QueryKey } from '@tanstack/vue-query'
import { onUnmounted, type Ref } from 'vue'
import { events } from '@/bindings'
import { listenForDetailChanges } from '@/lib/matchDetail'
import { useTauriEvents } from './useTauriEvents'

/** How long a match page waits for things to go quiet before refetching. */
export const DETAIL_REFRESH_DELAY_MS = 500

/**
 * Keeps a match page's detail fresh: a live match keeps changing, and a
 * removal, a deck change or a league change moves what the page shows.
 * Refetches `queryKey` once things go quiet, never cancelling a running
 * fetch.
 */
export function useDetailRefresh(token: Ref<string>, queryKey: () => QueryKey) {
  const queryClient = useQueryClient()

  let refreshTimer: ReturnType<typeof setTimeout> | null = null
  function scheduleRefresh() {
    if (refreshTimer !== null) clearTimeout(refreshTimer)
    refreshTimer = setTimeout(() => {
      refreshTimer = null
      void queryClient.invalidateQueries({ queryKey: queryKey() }, { cancelRefetch: false })
    }, DETAIL_REFRESH_DELAY_MS)
  }

  useTauriEvents(() => [listenForDetailChanges(events, () => token.value, scheduleRefresh)])
  onUnmounted(() => {
    if (refreshTimer !== null) clearTimeout(refreshTimer)
  })
}
