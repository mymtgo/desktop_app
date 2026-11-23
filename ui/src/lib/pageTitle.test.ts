import { describe, expect, it, vi } from 'vitest'
import { router } from '@/router'
import { PAGES, pageTitle as titleFor } from './pageTitle'

// The router builds a browser history on import; this test only reads its route names.
vi.mock('vue-router', async (importOriginal) => {
  const actual = await importOriginal<typeof import('vue-router')>()
  return { ...actual, createWebHistory: () => actual.createMemoryHistory() }
})

const APP_NAME = 'Qapp'
const pageTitle = (routeName: string | null, detail: string | null) => titleFor(routeName, detail, APP_NAME)

describe('pageTitle', () => {
  it('names a plain page, then the app, as 0.x does', () => {
    expect(pageTitle('matches', null)).toBe(`Matches - ${APP_NAME}`)
    expect(pageTitle('dashboard', null)).toBe(`Dashboard - ${APP_NAME}`)
    expect(pageTitle('settings-privacy', null)).toBe(`Data & Privacy - ${APP_NAME}`)
    expect(pageTitle('settings-archives', null)).toBe(`Archives - ${APP_NAME}`)
  })

  it('puts the event or deck name before the page, as 0.x does for Limited', () => {
    expect(pageTitle('limited-draft', 'FIN Draft')).toBe(`FIN Draft · Draft - ${APP_NAME}`)
    expect(pageTitle('limited-match', 'FIN Draft')).toBe(`FIN Draft · Match - ${APP_NAME}`)
    expect(pageTitle('deck-card-stats', 'Burn')).toBe(`Burn · Card Stats - ${APP_NAME}`)
  })

  it('falls back to the page alone until the name is known', () => {
    expect(pageTitle('limited-cards', null)).toBe(`Cards - ${APP_NAME}`)
  })

  it('falls back to the app name for a route it does not know', () => {
    expect(pageTitle('nope', null)).toBe(APP_NAME)
    expect(pageTitle(null, null)).toBe(APP_NAME)
  })

  it('has a page name for every named route in the router', () => {
    const names = router
      .getRoutes()
      .map((route) => route.name)
      .filter((name): name is string => typeof name === 'string')
    expect(names.length).toBeGreaterThan(30)
    const missing = names.filter((name) => !(name in PAGES))
    expect(missing).toEqual([])
  })
})
