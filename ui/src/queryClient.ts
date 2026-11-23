import { QueryClient } from '@tanstack/vue-query'

// Pushed events keep cached pages fresh (Task 10), so background refetches are
// rare: cached data renders at once (spec 6, cached data first).
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 30_000, refetchOnWindowFocus: false, retry: 1 },
  },
})
