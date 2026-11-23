import { useQuery } from '@tanstack/vue-query'
import { computed, type Ref } from 'vue'
import { commands } from '@/bindings'
import { unwrap } from '@/lib/commands'
import { matchDetailKeys } from '@/lib/matchDetail'
import { useDetailRefresh } from './useDetailRefresh'

/**
 * One match's detail for a match page (the global page and the one inside a
 * deck), refetched as the match and what it links to change.
 */
export function useLiveMatchDetail(token: Ref<string>) {
  const query = useQuery({
    queryKey: computed(() => matchDetailKeys.detail(token.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.matchDetail(queryKey[1])),
  })
  useDetailRefresh(token, () => matchDetailKeys.detail(token.value))
  return query
}
