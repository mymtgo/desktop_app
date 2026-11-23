import type { UnlistenFn } from '@tauri-apps/api/event'
import { onMounted, onUnmounted } from 'vue'

export interface TauriEvents {
  /**
   * Resolves once every listener is registered: true while the component is
   * still mounted, false when it unmounted first (the listeners are then
   * already removed). Work that needs the listeners in place, such as the
   * first read that a push may overtake, waits for it.
   */
  ready: Promise<boolean>
  /** Whether the component is still mounted (for checks after a later await). */
  isMounted: () => boolean
}

/**
 * Listens to app pushes for a component's lifetime:
 * `useTauriEvents(() => [events.a.listen(...), events.b.listen(...)])`.
 * The listeners are registered on mount and removed on unmount. A component
 * unmounted before `listen` resolves removes them as soon as they arrive, so
 * none is left behind.
 */
export function useTauriEvents(listen: () => Promise<UnlistenFn>[]): TauriEvents {
  let stops: UnlistenFn[] = []
  let unmounted = false
  let settle: (listening: boolean) => void = () => {}
  const ready = new Promise<boolean>((resolve) => (settle = resolve))

  onMounted(async () => {
    let registered: UnlistenFn[]
    try {
      registered = await Promise.all(listen())
    } catch (error) {
      settle(false)
      throw error
    }
    if (unmounted) {
      registered.forEach((stop) => stop())
      settle(false)
      return
    }
    stops = registered
    settle(true)
  })

  onUnmounted(() => {
    unmounted = true
    stops.forEach((stop) => stop())
    stops = []
  })

  return { ready, isMounted: () => !unmounted }
}
