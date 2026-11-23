import { computed } from 'vue'
import type { Router } from 'vue-router'
import { commands, events, type WhatsNewAnswer } from '@/bindings'
import { whatsNewUrl, whatsNewVersions } from '@/content/whats-new'
import { unwrap } from '@/lib/commands'
import { useAppInfo } from '@/lib/queries'

/** The What's new link for the status bar version and the Updates card, `null` without content. */
export function useWhatsNewUrl() {
  const { data: info } = useAppInfo()
  return computed(() => whatsNewUrl(info.value?.version))
}

type StartRouter = Pick<Router, 'isReady' | 'push'> & { currentRoute: { value: { name?: unknown } } }

async function ask(): Promise<WhatsNewAnswer> {
  try {
    return unwrap(await commands.whatsNewOnStart(whatsNewVersions()))
  } catch {
    // Startup failed or the seen version could not be saved: open nothing.
    return 'nothing'
  }
}

type WakeUps = {
  /** Resolves at the next import end or settings change, at once if one came since the last call. */
  next: () => Promise<void>
  stop: () => void
}

/**
 * Listens for the 0.x import's end and settings changes (the first-run
 * choice). Registered before the first ask, so one that lands between the
 * wait answer and the wait itself is kept, not lost.
 */
async function listenForWakeUps(): Promise<WakeUps> {
  let pending = false
  let wake: (() => void) | null = null
  const ring = () => {
    if (wake === null) {
      pending = true
    } else {
      const woken = wake
      wake = null
      woken()
    }
  }
  const stops = await Promise.all([
    events.importProgressChanged.listen((event) => {
      const phase = event.payload.progress.phase
      if (phase === 'finished' || phase === 'failed') ring()
    }),
    events.settingsChanged.listen(() => ring()),
  ])
  return {
    next: () => {
      if (pending) {
        pending = false
        return Promise.resolve()
      }
      return new Promise<void>((resolve) => {
        wake = resolve
      })
    },
    stop: () => stops.forEach((stop) => stop()),
  }
}

/**
 * Asked once by the main window after start (release spec section 7): opens
 * What's new when the app says so. Told to wait (the first-run choice is
 * open or a 0.x import runs), it asks again once that is over, so the page
 * never covers either.
 */
export async function askWhatsNewOnStart(router: StartRouter): Promise<void> {
  await router.isReady()
  const wakeUps = await listenForWakeUps()
  let answer: WhatsNewAnswer
  try {
    answer = await ask()
    while (answer === 'wait') {
      await wakeUps.next()
      answer = await ask()
    }
  } finally {
    wakeUps.stop()
  }
  if (answer === 'show' && router.currentRoute.value.name !== 'whats-new') {
    await router.push({ name: 'whats-new' })
  }
}
