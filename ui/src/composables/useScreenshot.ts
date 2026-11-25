import { toPng } from 'html-to-image'
import { computed, ref } from 'vue'
import { commands } from '@/bindings'
import { errorMessage, unwrap } from '@/lib/commands'
import { manaFontEmbedCss } from '@/lib/manaFontEmbed'
import { useToast } from './useToast'

/** Writes a PNG to the clipboard; throws when the clipboard refuses. */
export type ImageWriter = (png: Blob) => Promise<void>

/** The system clipboard (WebKitGTK may refuse images). */
async function clipboardWriter(png: Blob): Promise<void> {
  await navigator.clipboard.write([new ClipboardItem({ 'image/png': png })])
}

/** The bytes of a `data:` URL (base64). */
export function dataUrlBytes(dataUrl: string): Uint8Array {
  const base64 = dataUrl.slice(dataUrl.indexOf(',') + 1)
  const text = atob(base64)
  const bytes = new Uint8Array(text.length)
  for (let index = 0; index < text.length; index++) bytes[index] = text.charCodeAt(index)
  return bytes
}

/** The most passes `renderStable` makes. */
export const MAX_CAPTURE_PASSES = 4

/**
 * `toPng` until two passes in a row give the same length. On WebKit (macOS,
 * WebKitGTK) the images inside the SVG load while it draws, so one pass
 * leaves some of them blank; each later pass finds more of them loaded.
 * Chromium usually settles in two passes. Returns the last pass; a pass that
 * throws fails the capture.
 */
async function renderStable(element: HTMLElement, options: Parameters<typeof toPng>[1]): Promise<string> {
  let last = await toPng(element, options)
  for (let pass = 2; pass <= MAX_CAPTURE_PASSES; pass++) {
    const next = await toPng(element, options)
    const settled = next.length === last.length
    last = next
    if (settled) break
  }
  return last
}

/**
 * "Copy as image" (0.x `useScreenshot`, archetypes spec section 9): renders
 * an element with `html-to-image` at pixel ratio 2 and writes the PNG to
 * the clipboard, with 0.x's toasts. The bundled Mana face is embedded so
 * mana symbols draw in the image. When the clipboard refuses (WebKitGTK),
 * `saveOffered` turns on and `save()` writes the PNG through the save
 * dialog instead. The element carries `data-capturing` while it renders.
 */
export function useScreenshot(options: { writeImage?: ImageWriter } = {}) {
  const writeImage = options.writeImage ?? clipboardWriter
  const capturing = ref(false)
  const saving = ref(false)
  const pending = ref<{ bytes: Uint8Array; name: string } | null>(null)
  const saveOffered = computed(() => pending.value !== null)
  const { add } = useToast()

  async function capture(element: HTMLElement, fileName: string) {
    if (capturing.value) return
    capturing.value = true
    pending.value = null
    try {
      // Only the Mana face is embedded (mana symbols); the rest use system fonts.
      const fontEmbedCSS = await manaFontEmbedCss()
      // `data-capturing` while it renders: CSS can show what only the image
      // carries (the Wrapped watermark). html-to-image reads computed styles.
      element.setAttribute('data-capturing', '')
      let dataUrl: string
      try {
        dataUrl = await renderStable(element, { pixelRatio: 2, includeQueryParams: true, fontEmbedCSS })
      } finally {
        element.removeAttribute('data-capturing')
      }
      const bytes = dataUrlBytes(dataUrl)
      const blob = new Blob([bytes as BlobPart], { type: 'image/png' })
      try {
        await writeImage(blob)
        add({ type: 'success', title: 'Copied!', message: 'Screenshot copied to clipboard' })
      } catch {
        add({ type: 'error', title: 'Copy failed', message: 'Could not write to clipboard' })
        pending.value = { bytes, name: fileName }
      }
    } catch (error) {
      console.error('[Screenshot] Capture failed:', error)
      add({ type: 'error', title: 'Screenshot failed', message: 'Could not capture the element' })
    }
    capturing.value = false
  }

  /** "Save image": the last capture the clipboard refused. */
  async function save() {
    const image = pending.value
    if (!image || saving.value) return
    saving.value = true
    try {
      const saved = unwrap(await commands.saveImage(Array.from(image.bytes), image.name))
      if (saved) {
        add({ type: 'success', title: 'Saved', message: 'Image saved' })
        pending.value = null
      }
    } catch (error) {
      add({ type: 'error', title: 'Save failed', message: errorMessage(error) })
    } finally {
      saving.value = false
    }
  }

  return { capture, capturing, save, saving, saveOffered }
}
