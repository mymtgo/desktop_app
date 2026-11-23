import { computed } from 'vue'
import { useRoute } from 'vue-router'

/** The event's run key from the route (vue-router decodes the URL-encoded param). */
export function useLimitedKey() {
  const route = useRoute()
  return computed(() => {
    const key = route.params.key
    return Array.isArray(key) ? (key[0] ?? '') : (key ?? '')
  })
}
