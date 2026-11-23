import { spawnSync } from 'node:child_process'
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { cmd } from 'web-ext'
import { filesUnder, zipEntries } from './zip.ts'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const version = JSON.parse(readFileSync(resolve(root, 'package.json'), 'utf8')).version
const packages = resolve(root, 'dist', 'packages')

// What a Firefox reviewer needs to rebuild the extension: no node_modules, no dist.
const SOURCE_PATHS = ['package.json', 'package-lock.json', 'tsconfig.json', 'vitest.config.ts', 'README.md', 'scripts', 'src', 'icons']

function packageName(kind, version) {
  return `mymtgo-endstep-${kind}-${version}.zip`
}

function writeSourceZip() {
  const entries = SOURCE_PATHS.flatMap((path) => filesUnder(resolve(root, path))).map((file) => ({
    name: file.slice(root.length + 1).split('\\').join('/'),
    data: readFileSync(file),
  }))
  writeFileSync(join(packages, packageName('source', version)), zipEntries(entries))
}

const built = spawnSync(process.execPath, [resolve(root, 'scripts/build.mjs'), '--store'], { stdio: 'inherit' })
if (built.status !== 0) process.exit(built.status ?? 1)

rmSync(packages, { recursive: true, force: true })
mkdirSync(packages, { recursive: true })
for (const browser of ['chrome', 'firefox']) {
  await cmd.build(
    {
      sourceDir: resolve(root, 'dist', 'store', browser),
      artifactsDir: packages,
      filename: packageName(browser, version),
      overwriteDest: true,
    },
    { shouldExitProgram: false },
  )
}
writeSourceZip()
console.log('Wrote the store packages to ' + packages)
