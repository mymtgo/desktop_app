import { build } from 'vite'
import { cpSync, mkdirSync, rmSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { manifestFor } from '../src/manifest.ts'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const entries = {
  'page-hook': 'src/page/main.ts',
  relay: 'src/relay/main.ts',
  background: 'src/background/main.ts',
  popup: 'src/popup/main.ts',
}
// --store builds the files uploaded to the stores into dist/store; the default is the dev build into dist.
const mode = process.argv.includes('--store') ? 'store' : 'dev'
const distDir = mode === 'store' ? resolve(root, 'dist', 'store') : resolve(root, 'dist')
const version = JSON.parse(readFileSync(resolve(root, 'package.json'), 'utf8')).version

for (const browser of ['chrome', 'firefox']) {
  const outDir = resolve(distDir, browser)
  rmSync(outDir, { recursive: true, force: true })
  mkdirSync(outDir, { recursive: true })
  for (const [name, file] of Object.entries(entries)) {
    await build({
      root,
      configFile: false,
      logLevel: 'warn',
      build: {
        outDir,
        emptyOutDir: false,
        minify: false,
        lib: { entry: resolve(root, file), formats: ['iife'], name: 'mymtgo_' + name.replace('-', '_'), fileName: () => name + '.js' },
      },
    })
  }
  cpSync(resolve(root, 'icons'), resolve(outDir, 'icons'), { recursive: true })
  cpSync(resolve(root, 'src/popup/popup.html'), resolve(outDir, 'popup.html'))
  writeFileSync(resolve(outDir, 'manifest.json'), JSON.stringify(manifestFor(browser, version, mode), null, 2) + '\n')
}
