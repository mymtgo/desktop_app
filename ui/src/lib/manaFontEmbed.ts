// The Mana face for "Copy as image" (issue #326). html-to-image draws the
// element inside an SVG image, which cannot reach the page's fonts, and its
// own font embedding (every face on the page) is not used. So the bundled
// mana-font woff2 is handed over as `fontEmbedCSS`, a `data:` URL read once.
import manaWoff2Url from 'mana-font/fonts/mana.woff2?url'

/** Base64 of some bytes (in chunks, so a large font does not overflow the call stack). */
export function bytesToBase64(bytes: Uint8Array): string {
  let text = ''
  const chunk = 0x8000
  for (let start = 0; start < bytes.length; start += chunk) {
    text += String.fromCharCode(...bytes.subarray(start, start + chunk))
  }
  return btoa(text)
}

/** The `@font-face` rule for the Mana face with the font inline. */
export function manaFontFace(woff2: Uint8Array): string {
  return `@font-face { font-family: 'Mana'; src: url(data:font/woff2;base64,${bytesToBase64(woff2)}) format('woff2'); font-weight: normal; font-style: normal; }`
}

/** Reads the bundled font (the app's own asset, never the network). */
async function readBundledFont(): Promise<Uint8Array> {
  const response = await fetch(manaWoff2Url)
  if (!response.ok) throw new Error(`mana-font: ${response.status}`)
  return new Uint8Array(await response.arrayBuffer())
}

let cached: Promise<string> | null = null

/**
 * The CSS to embed in a screenshot so its mana symbols draw. Empty when the
 * font cannot be read (the capture still works, without symbols), and read
 * again next time.
 */
export function manaFontEmbedCss(read: () => Promise<Uint8Array> = readBundledFont): Promise<string> {
  if (!cached) {
    cached = read()
      .then(manaFontFace)
      .catch((error: unknown) => {
        console.error('[Screenshot] Could not read mana-font:', error)
        cached = null
        return ''
      })
  }
  return cached
}

/** Forgets the cached face (tests). */
export function resetManaFontEmbed() {
  cached = null
}
