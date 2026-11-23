import { describe, expect, it, vi } from 'vitest'

const state = vi.hoisted(() => ({ current: 'mtgo' as 'mtgo' | 'arena' | 'fail' }))
vi.mock('@/bindings', async (importOriginal) => {
  const original = await importOriginal<typeof import('@/bindings')>()
  return {
    ...original,
    commands: {
      ...original.commands,
      platformContext: async () => {
        if (state.current === 'fail') throw new Error('no backend')
        return {
          status: 'ok',
          data: { current: state.current, mtgo: true, arena: true, switchable: true },
        }
      },
    },
  }
})

// The app's router with an in-memory history: Node has no `window.history`.
vi.mock('vue-router', async (importOriginal) => {
  const original = await importOriginal<typeof import('vue-router')>()
  return { ...original, createWebHistory: () => original.createMemoryHistory() }
})

const { router } = await import('./router')
const { queryClient } = await import('@/queryClient')
const { keys } = await import('@/lib/keys')

describe('the platform guard', () => {
  it('a hidden route in the arena context lands on the dashboard', async () => {
    queryClient.clear()
    state.current = 'arena'
    await router.push('/leagues')
    expect(router.currentRoute.value.name).toBe('dashboard')
    await router.push('/limited/abc/draft')
    expect(router.currentRoute.value.name).toBe('limited-draft')
    await router.push('/decks/7/leagues')
    expect(router.currentRoute.value.name).toBe('dashboard')
    await router.push('/decks')
    expect(router.currentRoute.value.name).toBe('decks')
  })

  it('lets a route through in the mtgo context', async () => {
    queryClient.clear()
    state.current = 'mtgo'
    await router.push('/leagues')
    expect(router.currentRoute.value.name).toBe('leagues')
  })

  it('reads an invalidated context again before letting a route through', async () => {
    queryClient.clear()
    state.current = 'mtgo'
    await router.push('/decks')
    state.current = 'arena'
    await queryClient.invalidateQueries({ queryKey: keys.platformContext })
    await router.push('/leagues')
    expect(router.currentRoute.value.name).toBe('dashboard')
  })

  it('allows the route when the context cannot be read', async () => {
    queryClient.clear()
    state.current = 'fail'
    await router.push('/cards')
    expect(router.currentRoute.value.name).toBe('cards')
    expect(queryClient.getQueryData(keys.platformContext)).toBeUndefined()
  })
})

describe('the settings routes', () => {
  // 0.x tests/Feature/Settings/IndexControllerTest.php
  it('redirects the settings index to the general page', async () => {
    await router.push('/settings')
    expect(router.currentRoute.value.path).toBe('/settings/general')
  })

  it('nests the settings pages under the settings layout', () => {
    const settings = router.options.routes.find((route) => route.path === '/settings')
    const children = (settings?.children ?? []).map((child) => child.path)
    expect(children).toEqual([
      '',
      'general',
      'account',
      'overlays',
      'storage',
      'archives',
      'replays',
      'integrations',
      'privacy',
      'advanced',
      'advanced/licences',
    ])
    const layout = router.resolve('/settings/privacy').matched[0]
    expect(layout.path).toBe('/settings')
    expect(layout.components?.default).toBeDefined()
  })

  // League self-management spec 5.5: Archives, after Storage.
  it('routes /settings/archives to the Archives page', () => {
    const matched = router.resolve('/settings/archives')
    expect(matched.name).toBe('settings-archives')
    expect(matched.matched[0].path).toBe('/settings')
  })

  // Release spec section 9: the About card's "Third-party licences" page.
  it('puts the licences page under the settings layout', () => {
    const matched = router.resolve('/settings/advanced/licences')
    expect(matched.name).toBe('settings-licences')
    expect(matched.matched[0].path).toBe('/settings')
  })
})

describe('the What\'s new route', () => {
  // Release spec section 7: `/whats-new`, outside the settings layout.
  it('is /whats-new, named whats-new', () => {
    const matched = router.resolve('/whats-new')
    expect(matched.name).toBe('whats-new')
    expect(matched.matched).toHaveLength(1)
  })
})

describe('the metagame route', () => {
  it('takes a format key with an underscore (Arena Historic Brawl)', () => {
    const matched = router.resolve('/metagame/historic_brawl')
    expect(matched.name).toBe('metagame')
    expect(matched.params.format).toBe('historic_brawl')
  })
})

describe('the archetype route (issue #326)', () => {
  it('carries the tab in the path, Decklist when left out', () => {
    expect(router.resolve('/metagame/archetypes/12').name).toBe('archetype')
    expect(router.resolve('/metagame/archetypes/12').params.tab).toBe('')
    const matches = router.resolve('/metagame/archetypes/12/matches?timeframe=alltime')
    expect(matches.name).toBe('archetype')
    expect(matches.params).toEqual({ id: '12', tab: 'matches' })
    expect(matches.query.timeframe).toBe('alltime')
    expect(router.resolve('/metagame/archetypes/12/sideboard-guides').params.tab).toBe('sideboard-guides')
  })

  it('leaves the edit page its own route', () => {
    expect(router.resolve('/metagame/archetypes/12/edit').name).toBe('archetype-edit')
  })

  it('builds the side menu link with the tab and timeframe', () => {
    const resolved = router.resolve({ name: 'archetype', params: { id: '3', tab: 'sideboard-guides' }, query: { timeframe: 'last7' } })
    expect(resolved.fullPath).toBe('/metagame/archetypes/3/sideboard-guides?timeframe=last7')
  })
})

describe('a match inside a deck (issue #332)', () => {
  it('sits in the deck layout under /decks, apart from the global match page', () => {
    const inDeck = router.resolve('/decks/3/matches/q-token?version=4')
    expect(inDeck.name).toBe('deck-match')
    expect(inDeck.params).toEqual({ id: '3', token: 'q-token' })
    expect(inDeck.matched[0].path).toBe('/decks/:id')
    expect(inDeck.meta.deckTimeframe).toBe(true)
    expect(router.resolve('/decks/3/matches').name).toBe('deck-matches')
    expect(router.resolve('/matches/q-token').name).toBe('match')
  })
})
