import { existsSync, readdirSync, readFileSync } from 'node:fs'
import process from 'node:process'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { validate } from '@mtgreplay/mtgr'

const REPO = fileURLToPath(new URL('../../../', import.meta.url))
const FIXTURES = `${REPO}crates/mtgr/tests/fixtures`
const SNAPSHOTS = `${REPO}crates/arena-game/src/snapshots`
const ENDSTEP_SNAPSHOTS = `${REPO}crates/endstep-match/tests/snapshots`

/** Problem counts by check number for one .mtgr text. */
function problemCounts(text: string): Record<string, number> {
  const counts: Record<string, number> = {}
  for (const problem of validate(text.normalize('NFC'))) {
    const key = String(problem.check)
    counts[key] = (counts[key] ?? 0) + 1
  }
  return counts
}

/** The .mtgr text of an insta snapshot: everything after the second `---` line. */
function snapshotText(snap: string): string {
  const lines = snap.split('\n')
  let seen = 0
  for (const [i, line] of lines.entries()) {
    if (line === '---' && ++seen === 2) return lines.slice(i + 1).join('\n')
  }
  throw new Error('snapshot has no insta header')
}

function filesIn(folder: string, keep: (name: string) => boolean): string[] {
  if (!existsSync(folder)) return []
  return readdirSync(folder).filter(keep).sort()
}

describe('conformance fixtures', () => {
  for (const name of filesIn(FIXTURES, (n) => n.endsWith('.mtgr'))) {
    it(`${name} has no problems`, () => {
      expect(problemCounts(readFileSync(`${FIXTURES}/${name}`, 'utf8'))).toEqual({})
    })
  }
})

describe.each([
  ['Arena', SNAPSHOTS],
  ['Endstep', ENDSTEP_SNAPSHOTS],
])('%s goldens', (platform, folder) => {
  const goldens = filesIn(folder, (n) => n.includes('mtgr') && n.endsWith('.snap'))

  it('has goldens to validate', () => {
    expect(goldens.length).toBeGreaterThan(platform === 'Endstep' ? 1 : 0)
  })

  for (const name of goldens) {
    it(`${name} has no problems`, () => {
      const text = snapshotText(readFileSync(`${folder}/${name}`, 'utf8'))
      const problems = validate(text.normalize('NFC')).map((p) => `${p.line} check ${p.check}: ${p.message}`)
      if (problems.length > 0) console.log(`${name}\n${problems.join('\n')}`)
      expect(problems).toHaveLength(0)
    })
  }
})

describe.skipIf(!process.env.MTGR_VALIDATE_DIR)('MTGR_VALIDATE_DIR', () => {
  it('validates every .mtgr file there with no problems allowed', () => {
    const dir = process.env.MTGR_VALIDATE_DIR as string
    const failures: string[] = []
    for (const name of filesIn(dir, (n) => n.endsWith('.mtgr'))) {
      const problems = validate(readFileSync(`${dir}/${name}`, 'utf8').normalize('NFC'))
      for (const p of problems) failures.push(`${name}:${p.line} check ${p.check}: ${p.message}`)
    }
    if (failures.length > 0) console.log(failures.join('\n'))
    expect(failures).toEqual([])
  })
})
