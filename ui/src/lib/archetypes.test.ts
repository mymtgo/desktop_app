import { describe, expect, it } from 'vitest'
import type { ListCardView } from '@/bindings'
import {
  ARCHETYPE_FORMATS,
  MANAGE_FILTERS_KEY,
  archetypeDetailPath,
  archetypeEditPath,
  colorLetters,
  createArchetypeRoute,
  formProblem,
  groupListCards,
  manageFromUrl,
  manageUrlQuery,
  readRememberedFilters,
  sourceBadge,
  submitButtonLabel,
  toggleColor,
  writeRememberedFilters, mergedNotice } from './archetypes'

describe('the archetype form (0.x ArchetypeForm)', () => {
  it('offers the seven formats in 0.x order', () => {
    expect(ARCHETYPE_FORMATS.map((format) => format.label)).toEqual([
      'Standard',
      'Pioneer',
      'Modern',
      'Pauper',
      'Legacy',
      'Vintage',
      'Premodern',
    ])
  })

  it('validates the name, format and cards as the server does', () => {
    const ok = { name: 'Qburn', format: 'modern', cardCount: 1, requireCards: true }
    expect(formProblem(ok)).toBeNull()
    expect(formProblem({ ...ok, name: '  ' })).toBe('The name field is required.')
    expect(formProblem({ ...ok, name: 'Q'.repeat(256) })).toBe(
      'The name field must not be greater than 255 characters.',
    )
    expect(formProblem({ ...ok, name: 'Q'.repeat(255) })).toBeNull()
    expect(formProblem({ ...ok, format: '' })).toBe('The format field is required.')
    expect(formProblem({ ...ok, cardCount: 0 })).toBe('The cards field is required.')
    expect(formProblem({ ...ok, cardCount: 0, requireCards: false })).toBeNull()
  })

  it('labels the edit button "Save & Add Variant" when cards are loaded', () => {
    expect(submitButtonLabel(true, 3, 'Create Archetype')).toBe('Create Archetype')
    expect(submitButtonLabel(false, 0, 'Save Changes')).toBe('Save Changes')
    expect(submitButtonLabel(false, 3, 'Save Changes')).toBe('Save & Add Variant')
  })
})

describe('the colour toggle (0.x ManaIdentityPicker)', () => {
  it('toggles a colour and keeps WUBRG order with colourless last', () => {
    expect(toggleColor(null, 'R')).toBe('R')
    expect(toggleColor('R', 'W')).toBe('W,R')
    expect(toggleColor('W,R', 'C')).toBe('W,R,C')
    expect(toggleColor('W,R', 'W')).toBe('R')
    expect(toggleColor('R', 'R')).toBeNull()
  })

  it('reads the letters of a stored identity', () => {
    expect(colorLetters('U,B')).toEqual(['U', 'B'])
    expect(colorLetters(null)).toEqual([])
    expect(colorLetters('')).toEqual([])
  })
})

describe('the manage query (0.x StickyFiltersTest)', () => {
  it('round trips through the URL', () => {
    const state = { search: 'mono', format: 'modern', source: 'manual' as const, page: 2 }
    expect(manageFromUrl(manageUrlQuery(state), null).state).toEqual(state)
    expect(manageUrlQuery({ search: '', format: '', source: '', page: 1 })).toEqual({ search: '', format: '', source: '' })
  })

  it('filters by source: custom, server or every source', () => {
    expect(manageFromUrl({ source: 'manual' }, null).state.source).toBe('manual')
    expect(manageFromUrl({ source: 'downloaded' }, null).state.source).toBe('downloaded')
    expect(manageFromUrl({ source: 'commander' }, null).state.source).toBe('commander')
    expect(manageFromUrl({ source: 'fallback' }, null).state.source).toBe('')
    expect(manageFromUrl({ search: '' }, null).state.source).toBe('')
  })

  it('keeps the list filtered when a page opens with no query', () => {
    const remembered = { search: 'mono', format: 'modern', source: 'manual' as const }
    const { state, remember } = manageFromUrl({}, remembered)
    expect(state).toEqual({ search: 'mono', format: 'modern', source: 'manual', page: 1 })
    expect(remember).toBeNull()
  })

  it('clears the remembered filters when the user empties them', () => {
    const { state, remember } = manageFromUrl(
      { search: '', format: '', source: '' },
      { search: 'mono', format: 'modern', source: 'manual' },
    )
    expect(state).toEqual({ search: '', format: '', source: '', page: 1 })
    expect(remember).toEqual({ search: '', format: '', source: '' })
  })

  it('replaces the remembered filters when a new filter is applied', () => {
    const { state, remember } = manageFromUrl(
      { search: 'azorius', format: 'legacy' },
      { search: 'mono', format: 'modern', source: '' },
    )
    expect(state.search).toBe('azorius')
    expect(remember).toEqual({ search: 'azorius', format: 'legacy', source: '' })
  })

  // 0.x "does not let a partial reload overwrite the remembered filters":
  // the create form's match select has its own state, so only the manage
  // page's own query is ever remembered.
  it('remembers in localStorage and survives junk there', () => {
    const store = new Map<string, string>()
    const storage = {
      getItem: (key: string) => store.get(key) ?? null,
      setItem: (key: string, value: string) => void store.set(key, value),
    }
    expect(readRememberedFilters(storage)).toBeNull()
    writeRememberedFilters(storage, { search: 'mono', format: 'modern', source: 'manual' })
    expect(readRememberedFilters(storage)).toEqual({ search: 'mono', format: 'modern', source: 'manual' })
    // Filters remembered before the source filter existed.
    store.set(MANAGE_FILTERS_KEY, '{"search":"mono","format":"modern"}')
    expect(readRememberedFilters(storage)).toEqual({ search: 'mono', format: 'modern', source: '' })
    store.set(MANAGE_FILTERS_KEY, '{not json')
    expect(readRememberedFilters(storage)).toBeNull()
    store.set(MANAGE_FILTERS_KEY, '{"search":5}')
    expect(readRememberedFilters(storage)).toBeNull()
  })

  it('forgets nothing loudly when storage is blocked or full', () => {
    const blocked = {
      getItem: (): string | null => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('quota exceeded')
      },
    }
    expect(readRememberedFilters(blocked)).toBeNull()
    expect(() => writeRememberedFilters(blocked, { search: '', format: '', source: '' })).not.toThrow()
  })
})

describe('archetype routes', () => {
  it('opens the create form, optionally from a match (T8)', () => {
    expect(createArchetypeRoute()).toEqual({ path: '/metagame/archetypes/new', query: {} })
    expect(createArchetypeRoute({ sourceMatchId: 7 })).toEqual({
      path: '/metagame/archetypes/new',
      query: { source_match_id: '7' },
    })
    expect(createArchetypeRoute({ format: 'modern' })).toEqual({
      path: '/metagame/archetypes/new',
      query: { format: 'modern' },
    })
    expect(archetypeDetailPath(3)).toBe('/metagame/archetypes/3')
    expect(archetypeEditPath(3)).toBe('/metagame/archetypes/3/edit')
  })

  it('badges manual and fallback archetypes as 0.x does', () => {
    expect(sourceBadge('manual', false, false)).toBe('Custom')
    expect(sourceBadge('fallback', false, false)).toBe('System')
    expect(sourceBadge('commander', false, false)).toBe('Commander')
    expect(sourceBadge('downloaded', true, false)).toBe('Retired')
    expect(sourceBadge('downloaded', false, false)).toBeNull()
  })

  it('shows no Custom badge on a server archetype held by the offline download', () => {
    expect(sourceBadge('manual', false, true)).toBeNull()
    expect(sourceBadge('manual', true, true)).toBe('Retired')
  })
})

describe('a variant decklist', () => {
  const card = (catalogId: number, typeLine: string | null, sideboard: boolean): ListCardView => ({
    catalogId,
    name: `Qcard ${catalogId}`,
    typeLine,
    colors: [],
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    cmc: null,
    manaCost: null,
    quantity: 2,
    sideboard,
    known: typeLine !== null,
  })

  it('splits the sideboard off and groups the maindeck by type line', () => {
    const { maindeck, sideboard } = groupListCards([card(1, 'Creature', false), card(2, 'Instant', true)])
    expect(maindeck).toEqual([{ typeName: 'Creature', count: 2, cards: [card(1, 'Creature', false)] }])
    expect(sideboard).toEqual([card(2, 'Instant', true)])
  })
})

describe('the merged notice (0.x ShowMergedArchetypeTest)', () => {
  it('reads exactly as 0.x', () => {
    expect(mergedNotice('Qparent')).toEqual({
      before: 'This archetype has been merged into ',
      name: 'Qparent',
      after: '. Future detections will be attributed to it.',
    })
  })
})
