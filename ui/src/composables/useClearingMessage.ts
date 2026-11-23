import { getCurrentInstance, onUnmounted, ref } from 'vue'

/** How long an inline error stays before it goes away on its own. */
export const CLEARING_MESSAGE_MS = 6000

/**
 * An inline message that clears itself: a failure shown beside a button
 * should not stay for good after the player has moved on.
 */
export function useClearingMessage() {
  const message = ref<string | null>(null)
  let timer: ReturnType<typeof setTimeout> | null = null

  function clear() {
    if (timer) clearTimeout(timer)
    timer = null
    message.value = null
  }

  function show(text: string) {
    clear()
    message.value = text
    timer = setTimeout(clear, CLEARING_MESSAGE_MS)
  }

  if (getCurrentInstance()) onUnmounted(clear)
  return { message, show, clear }
}
