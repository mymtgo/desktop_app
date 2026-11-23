import { describe, expect, it } from 'vitest'
import type { ArchetypeCatalogOption, ArchetypePickerOption } from '@/bindings'
import { archetypeFormatKey, bulkFormat, pickerRows, setArchetypeDescription, splitArchetypes } from './archetypePicker'

function option(id: number, name: string, format: string | null, isFallback = false, isCustom = false): ArchetypePickerOption {
  return { id, name, format, colors: [], isFallback, isCustom }
}

const OPTIONS = [
  option(1, 'Homebrew', null, true),
  option(2, 'Rogue', null, true),
  option(3, 'Qburn', 'modern'),
  option(4, 'Qtron', 'modern'),
  option(5, 'Qbelcher', 'legacy'),
]

describe('archetypeFormatKey (0.x MtgoFormat::key)', () => {
  it('drops the constructed C before a capital and lowercases', () => {
    expect(archetypeFormatKey('CMODERN')).toBe('modern')
    expect(archetypeFormatKey('CModern')).toBe('modern')
    expect(archetypeFormatKey('Modern')).toBe('modern')
    expect(archetypeFormatKey('Commander')).toBe('commander')
  })
})

describe('splitArchetypes (0.x useArchetypeSplit)', () => {
  it('keeps the format and every fallback, the fallbacks apart', () => {
    const { fallbacks, regular } = splitArchetypes(OPTIONS, 'CMODERN', '')
    expect(fallbacks.map((a) => a.name)).toEqual(['Homebrew', 'Rogue'])
    expect(regular.map((a) => a.name)).toEqual(['Qburn', 'Qtron'])
  })

  it('takes a format label too, and no format keeps every archetype', () => {
    expect(splitArchetypes(OPTIONS, 'Legacy', '').regular.map((a) => a.name)).toEqual(['Qbelcher'])
    expect(splitArchetypes(OPTIONS, null, '').regular).toHaveLength(3)
  })

  it('searches names in any case; the fallbacks always show', () => {
    const { fallbacks, regular } = splitArchetypes(OPTIONS, 'CMODERN', ' TRO ')
    expect(regular.map((a) => a.name)).toEqual(['Qtron'])
    expect(fallbacks).toHaveLength(2)
  })
})

describe('pickerRows (#489: the server catalog in the picker)', () => {
  const held = [option(1, 'Qdimir Frog', 'modern')]
  const catalog: ArchetypeCatalogOption[] = [
    { uuid: 'q-reanimator', name: 'Qgrixis Reanimator', format: 'modern', colors: ['U', 'B', 'R'], colorIdentity: 'U,B,R' },
    { uuid: 'q-oculus', name: 'Qdimir Oculus', format: 'modern', colors: ['U', 'B'], colorIdentity: 'U,B' },
  ]

  it('lists held and unmet archetypes together by name', () => {
    expect(pickerRows(held, catalog, '').map((row) => [row.name, row.id])).toEqual([
      ['Qdimir Frog', 1],
      ['Qdimir Oculus', null],
      ['Qgrixis Reanimator', null],
    ])
  })

  it('searches the unmet ones as the held ones', () => {
    expect(pickerRows(held, catalog, 'DIMIR').map((row) => row.name)).toEqual(['Qdimir Frog', 'Qdimir Oculus'])
  })

  // A player's own archetype can share its name with MyMTGO's: the badge tells them apart.
  it('marks the player\'s own archetypes as custom, never a server one', () => {
    const own = [option(7, 'Qdimir Oculus', 'modern', false, true)]
    expect(pickerRows(own, catalog, 'oculus').map((row) => [row.key, row.isCustom])).toEqual([
      ['held:7', true],
      ['server:q-oculus', false],
    ])
  })

  it('without a catalog lists the held ones only', () => {
    expect(pickerRows(held, undefined, '').map((row) => row.key)).toEqual(['held:1'])
  })
})

describe('the Set Archetype dialog', () => {
  it('says how many matches a bulk set changes (0.x SetArchetypeDialog)', () => {
    expect(setArchetypeDescription(1)).toBe('Set archetype for 1 selected match.')
    expect(setArchetypeDescription(3)).toBe('Set archetype for 3 selected matches.')
    expect(setArchetypeDescription(null)).toBe('Search and select an archetype for this opponent.')
  })

  it('narrows a bulk set to the selection format only when they share one', () => {
    expect(bulkFormat(['Modern', 'Modern'])).toBe('Modern')
    expect(bulkFormat(['Modern', 'Legacy'])).toBeNull()
    expect(bulkFormat([])).toBeNull()
  })
})
