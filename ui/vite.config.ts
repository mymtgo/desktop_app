/// <reference types="vitest/config" />
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig, type Plugin } from 'vite'

/**
 * mana-font's stylesheet (`src/main.ts` and the other entries import it)
 * offers eot, woff, ttf and svg for its Mana face, and a second face
 * (MPlantin) no class here uses, so Vite would bundle all ten files. Its
 * `@font-face` rules are swapped for one Mana face with the package's woff2
 * (as the website does), so that is the only font file shipped. The build
 * fails when the stylesheet is not the shape this expects (exactly the Mana
 * and MPlantin faces) or was never seen, rather than shipping every file.
 */
function manaFontWoff2Only(): Plugin {
  const FACE = /@font-face\s*\{[^}]*\}/g
  let building = false
  let seen = false
  return {
    name: 'mana-font-woff2-only',
    enforce: 'pre',
    configResolved(config) {
      building = config.command === 'build'
    },
    async transform(code, id) {
      if (!/mana-font[\\/]css[\\/]mana\.css$/.test(id)) return null
      const faces = code.match(FACE) ?? []
      const families = faces.map((face) => /font-family:\s*"?([^";]+)"?/.exec(face)?.[1] ?? '?')
      if (families.join(',') !== 'Mana,MPlantin') {
        this.error(`mana-font: expected the Mana and MPlantin @font-face rules, found [${families.join(', ')}]`)
      }
      seen = true
      const classes = code.replace(FACE, '')
      if (classes.includes('url(')) this.error('mana-font: the stylesheet names files outside its @font-face rules')
      if (!(await this.resolve('../fonts/mana.woff2', id))) this.error('mana-font: fonts/mana.woff2 is missing')
      const face =
        "@font-face { font-family: 'Mana'; src: url('../fonts/mana.woff2') format('woff2'); font-weight: normal; font-style: normal; font-display: block; }\n"
      return { code: face + classes, map: null }
    },
    buildEnd(error) {
      if (building && !error && !seen) {
        this.error('mana-font: mana-font/css/mana.css was never bundled, so its fonts were not swapped')
      }
    },
  }
}

export default defineConfig({
  plugins: [manaFontWoff2Only(), vue(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      '@mtgreplay/mtgr': fileURLToPath(new URL('./src/vendor/mtgr/index.ts', import.meta.url)),
    },
  },
  server: {
    // Tauri's dev command expects this exact port (app/tauri.conf.json devUrl).
    port: 5173,
    strictPort: true,
    // The About card bundles the repository's LICENSE (outside `ui/`) as raw
    // text. Vite checks the `?raw` id against each entry, which only a folder
    // matches, so the repository folder (it holds `ui/`) is allowed; Vite's
    // deny list (`.env`, `.git`, keys) still holds.
    fs: { allow: [fileURLToPath(new URL('..', import.meta.url))] },
  },
  clearScreen: false,
  build: {
    target: 'es2022',
    // Four entries: the main window, the overlay windows' page
    // (`overlay.html?view=…`), the replay window's (`replay.html?game=…`)
    // and the data console's (`console.html`). Other windows add theirs here.
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        overlay: fileURLToPath(new URL('./overlay.html', import.meta.url)),
        replay: fileURLToPath(new URL('./replay.html', import.meta.url)),
        console: fileURLToPath(new URL('./console.html', import.meta.url)),
      },
    },
  },
  test: {
    // `mana.test.ts` and `tableDividers.test.ts` read stylesheets (`?raw`),
    // which Vitest otherwise empties.
    css: { include: [/mana-font[\\/]css[\\/]mana\.css/, /src[\\/]css[\\/]app\.css/] },
    projects: [
      { extends: true, test: { name: 'unit', include: ['src/**/*.test.ts'], exclude: ['src/**/*.client.test.ts'] } },
      // Component tests that mount `.vue` files: compiled as the window
      // compiles them (Vitest's node environment compiles them for server
      // rendering only), still in Node with no DOM library.
      {
        extends: true,
        test: {
          name: 'client',
          include: ['src/**/*.client.test.ts'],
          environment: './src/replay/clientTestEnvironment.ts',
        },
      },
    ],
  },
})
