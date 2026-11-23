import { nextTick, ref, shallowRef } from 'vue'
import { commands, type DeckScreenshotView } from '@/bindings'
import { unwrap } from '@/lib/commands'
import { useScreenshot, type ImageWriter } from './useScreenshot'
import { useToast } from './useToast'

/** A rendered component (its `$el`) or an element. */
type Target = { $el?: unknown } | HTMLElement | null

function elementOf(target: Target): HTMLElement | null {
  if (!target) return null
  if ('$el' in target) return (target.$el as HTMLElement | undefined) ?? null
  return target as HTMLElement
}

/** Resolves once every image under `element` has loaded (or failed) and decoded. */
export async function imagesReady(element: HTMLElement): Promise<void> {
  if (typeof element.querySelectorAll !== 'function') return
  const images = Array.from(element.querySelectorAll('img'))
  for (const image of images) {
    if (!image.complete) {
      await new Promise<void>((resolve) => {
        image.addEventListener('load', () => resolve(), { once: true })
        image.addEventListener('error', () => resolve(), { once: true })
      })
    }
    if (typeof image.decode === 'function') await image.decode().catch(() => undefined)
  }
}

/**
 * A screenshot rendered off screen then captured (0.x `Decklist`,
 * `LeagueCard`, `LeagueRunCard`, `DeckCardStats`): `take(data, fileName)`
 * puts `data` in `shot` so the page renders the screenshot component (bound
 * to `target`), waits for its images, captures it with `useScreenshot`
 * (0.x's toasts, "Save image" when the clipboard refuses), then clears it.
 */
export function useOffscreenScreenshot<T>(options: { writeImage?: ImageWriter } = {}) {
  const screenshot = useScreenshot(options)
  const shot = shallowRef<T | null>(null)
  const target = ref<Target>(null)
  const busy = ref(false)

  /**
   * Run one screenshot: `load` gives the data (or `null` to stop, having
   * said why). One at a time.
   */
  async function take(load: () => Promise<T | null>, fileName: string): Promise<void> {
    if (busy.value) return
    busy.value = true
    try {
      const data = await load()
      if (data === null) return
      shot.value = data
      await nextTick()
      const element = elementOf(target.value as Target)
      if (!element) return
      await imagesReady(element)
      await screenshot.capture(element, fileName)
    } finally {
      shot.value = null
      busy.value = false
    }
  }

  return {
    shot,
    target,
    busy,
    take,
    save: screenshot.save,
    saving: screenshot.saving,
    saveOffered: screenshot.saveOffered,
  }
}

/** The deck screenshot's data, or `null` after 0.x's "Could not load deck data" toast. */
export async function loadDeckScreenshot(deckId: number): Promise<DeckScreenshotView | null> {
  try {
    return unwrap(await commands.deckScreenshotData(deckId))
  } catch {
    useToast().add({ type: 'error', title: 'Screenshot failed', message: 'Could not load deck data' })
    return null
  }
}

/** The league screenshot's cover art; `null` when it cannot be read (0.x goes on without it). */
export async function loadLeagueCover(deckId: number | null): Promise<string | null> {
  if (deckId === null) return null
  try {
    return unwrap(await commands.leagueScreenshotData(deckId)).coverArtBase64
  } catch {
    return null
  }
}

/** A card stat screenshot's image; `null` without an oracle id or when it cannot be read (0.x). */
export async function loadCardImage(oracleId: string | null): Promise<string | null> {
  if (!oracleId) return null
  try {
    return unwrap(await commands.cardImageDataUrl(oracleId))
  } catch {
    return null
  }
}
