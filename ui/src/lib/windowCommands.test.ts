import { describe, expect, it } from 'vitest'
import overlayCapability from '../../../app/capabilities/overlay.json?raw'
import replayCapability from '../../../app/capabilities/replay.json?raw'

// Each window may call only the app commands its capability grants (#389):
// a command a page calls without a grant fails in that window. These walk
// each window's imports from its entry and check every `commands.x` call.

const sources = import.meta.glob<string>(['/src/**/*.ts', '/src/**/*.vue', '!/src/**/*.test.ts'], {
  eager: true,
  query: '?raw',
  import: 'default',
})

const IMPORT = /(?:import|export)[^'"]*?from\s*['"]([^'"]+)['"]|import\(\s*['"]([^'"]+)['"]\s*\)|import\s+['"]([^'"]+)['"]/g
const COMMAND = /commands\.([a-zA-Z0-9]+)/g

function directory(file: string): string {
  return file.slice(0, file.lastIndexOf('/'))
}

/** `/src/a/../b` as `/src/b`. */
function normalise(path: string): string {
  const parts: string[] = []
  for (const part of path.split('/')) {
    if (part === '..') parts.pop()
    else if (part !== '.' && part !== '') parts.push(part)
  }
  return `/${parts.join('/')}`
}

/** The source file an import names, or `null` for a package or asset. */
function resolve(from: string, specifier: string): string | null {
  let base: string
  if (specifier.startsWith('@/')) base = `/src/${specifier.slice(2)}`
  else if (specifier.startsWith('.')) base = normalise(`${directory(from)}/${specifier}`)
  else return null
  for (const candidate of [base, `${base}.ts`, `${base}.vue`, `${base}/index.ts`]) {
    if (candidate in sources) return candidate
  }
  return null
}

/** The commands called by the files an entry reaches, as Tauri names them. */
function commandsCalledFrom(entry: string): string[] {
  const seen = new Set<string>()
  const called = new Set<string>()
  const pending = [entry]
  while (pending.length > 0) {
    const file = pending.pop() as string
    if (seen.has(file) || file === '/src/bindings.ts') continue
    seen.add(file)
    const text = sources[file]
    for (const match of text.matchAll(IMPORT)) {
      const target = resolve(file, match[1] ?? match[2] ?? match[3])
      if (target) pending.push(target)
    }
    for (const match of text.matchAll(COMMAND)) {
      called.add(match[1].replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`))
    }
  }
  return [...called].sort()
}

function granted(capability: string): string[] {
  const permissions: string[] = JSON.parse(capability).permissions
  return permissions
    .filter((permission) => permission.startsWith('allow-'))
    .map((permission) => permission.slice('allow-'.length).replace(/-/g, '_'))
}

const bindings = sources['/src/bindings.ts']

describe('window command grants', () => {
  for (const [entry, capability] of [
    ['/src/overlay/main.ts', overlayCapability],
    ['/src/replay/main.ts', replayCapability],
  ] as const) {
    it(`grants ${entry} every command it calls`, () => {
      const called = commandsCalledFrom(entry)
      expect(called.length).toBeGreaterThan(0)
      for (const name of called) expect(bindings).toContain(`("${name}"`)
      expect(called.filter((name) => !granted(capability).includes(name))).toEqual([])
    })
  }
})
