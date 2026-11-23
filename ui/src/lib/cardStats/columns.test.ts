import { describe, expect, it } from 'vitest'
import {
  CARD_STATS_COLUMNS,
  CASTING_METHOD_COLUMNS,
  configurableColumns,
  DEFAULT_VISIBILITY,
  effectiveVisibility,
  loadVisibility,
  LOCAL_ONLY_COLUMNS,
  pickerGroups,
  saveVisibility,
  VISIBILITY_STORAGE_KEY,
} from './columns'

function storage(initial?: string): Pick<Storage, 'getItem' | 'setItem'> & { saved: Record<string, string> } {
  const saved: Record<string, string> = initial === undefined ? {} : { [VISIBILITY_STORAGE_KEY]: initial }
  return {
    saved,
    getItem: (key) => saved[key] ?? null,
    setItem: (key, value) => {
      saved[key] = value
    },
  }
}

describe('default visibility', () => {
  it('hides every casting method column and shows the rest', () => {
    for (const column of CASTING_METHOD_COLUMNS) {
      expect(DEFAULT_VISIBILITY[column.key]).toBe(false)
    }
    expect(DEFAULT_VISIBILITY.keptPct).toBe(true)
    expect(DEFAULT_VISIBILITY.games).toBe(true)
    expect(CASTING_METHOD_COLUMNS).toHaveLength(19)
  })
})

describe('loadVisibility', () => {
  it('starts from the defaults with nothing stored', () => {
    expect(loadVisibility(storage())).toEqual(DEFAULT_VISIBILITY)
  })

  it('merges what is stored over the defaults and ignores unknown keys', () => {
    const loaded = loadVisibility(storage(JSON.stringify({ games: false, warp: true, bogus: true, type: 'yes' })))
    expect(loaded.games).toBe(false)
    expect(loaded.warp).toBe(true)
    expect(loaded.type).toBe(true)
    expect('bogus' in loaded).toBe(false)
  })

  it('falls back to the defaults for broken storage', () => {
    expect(loadVisibility(storage('{nope'))).toEqual(DEFAULT_VISIBILITY)
    expect(loadVisibility(storage('7'))).toEqual(DEFAULT_VISIBILITY)
  })

  it('saves under cardStatsVisibleColumns', () => {
    const store = storage()
    saveVisibility(store, { ...DEFAULT_VISIBILITY, games: false })
    expect(VISIBILITY_STORAGE_KEY).toBe('cardStatsVisibleColumns')
    expect(JSON.parse(store.saved.cardStatsVisibleColumns).games).toBe(false)
  })
})

describe('their cards', () => {
  it('hides the local-only columns', () => {
    const effective = effectiveVisibility(DEFAULT_VISIBILITY, 'theirs')
    for (const key of LOCAL_ONLY_COLUMNS) {
      expect(effective[key]).toBe(false)
    }
    expect(effective.castPct).toBe(true)
    expect(effectiveVisibility(DEFAULT_VISIBILITY, 'mine')).toEqual(DEFAULT_VISIBILITY)
  })

  it('leaves the local-only columns out of the picker', () => {
    const keys = pickerGroups('theirs').flatMap((group) => group.columns.map((c) => c.key))
    expect(keys).not.toContain('sb')
    expect(keys).not.toContain('sbInPct')
    expect(keys).toContain('castPct')
  })
})

describe('Bo1 mode', () => {
  const SIDEBOARD = ['sb', 'sbOutPct', 'sbInPct']

  it('hides the sideboard columns whatever was chosen', () => {
    const effective = effectiveVisibility(DEFAULT_VISIBILITY, 'mine', true)
    for (const key of SIDEBOARD) expect(effective[key as keyof typeof effective]).toBe(false)
    expect(effective.keptPct).toBe(true)
  })

  it('leaves them out of the picker', () => {
    const keys = configurableColumns('mine', true).map((c) => c.key)
    for (const key of SIDEBOARD) expect(keys).not.toContain(key)
    expect(keys).toContain('keptPct')
    const grouped = pickerGroups('mine', true).flatMap((group) => group.columns.map((c) => c.key))
    for (const key of SIDEBOARD) expect(grouped).not.toContain(key)
  })

  it('changes nothing when off', () => {
    expect(effectiveVisibility(DEFAULT_VISIBILITY, 'mine', false)).toEqual(DEFAULT_VISIBILITY)
    expect(configurableColumns('mine', false)).toEqual(CARD_STATS_COLUMNS)
  })
})

describe('pickerGroups', () => {
  it('groups the columns and sorts each group by label', () => {
    const groups = pickerGroups('mine')
    expect(groups.map((g) => g.label)).toEqual(['Card info', 'Game metrics', 'Casting costs'])
    const casting = groups[2].columns.map((c) => c.label)
    expect(casting).toEqual([...casting].sort((a, b) => a.localeCompare(b)))
    expect(groups.flatMap((g) => g.columns)).toHaveLength(CARD_STATS_COLUMNS.length)
  })
})
