// The relay runs in the isolated world. After an extension update the declared content script and
// the re-injected relay.js can both run in one world, and every relay forwards every page message,
// so two live relays would double each frame. A probe on the global says whether an earlier relay
// is still alive; a relay whose extension context died answers false, so it never blocks a new one.
export const RELAY_PROBE = '__mymtgoRelay'

export interface RelayRuntime {
  readonly id: string | undefined
  sendMessage(message: unknown): unknown
}

export interface RelayTarget {
  addEventListener(type: 'message', listener: (event: MessageEvent) => void): void
}

// existing is whatever an earlier relay left on the global. Install unless it is a probe that says live.
export function shouldInstallRelay(existing: unknown): boolean {
  if (typeof existing !== 'function') return true
  try {
    return (existing as () => unknown)() !== true
  } catch {
    return true
  }
}

function isAlive(runtime: RelayRuntime): boolean {
  try {
    return runtime.id !== undefined
  } catch {
    return false
  }
}

export function installRelay(scope: Record<string, unknown>, target: RelayTarget, runtime: RelayRuntime): void {
  if (!shouldInstallRelay(scope[RELAY_PROBE])) return
  scope[RELAY_PROBE] = () => isAlive(runtime)
  target.addEventListener('message', (event: MessageEvent) => {
    try {
      if (event.source !== (target as unknown)) return
      const data: unknown = event.data
      if (typeof data !== 'object' || data === null) return
      if ((data as { source?: unknown }).source !== 'mymtgo-endstep') return
      const payload = (data as { event?: unknown }).event
      const sent = runtime.sendMessage({ endstep: payload })
      // MV3 returns a promise; it rejects when the extension was reloaded.
      Promise.resolve(sent).catch(() => {})
    } catch {
      // extension context gone: nothing to forward to
    }
  })
}
