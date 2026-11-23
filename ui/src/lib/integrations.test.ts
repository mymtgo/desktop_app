import { describe, expect, it } from 'vitest'
import type { ConnectionView, EndstepStatusView } from '@/bindings'
import {
  ARENA_OFF_TEXT,
  ARENA_ON_TEXT,
  agoText,
  browserLabel,
  browserInstalls,
  connectionText,
  isStoreListing,
  STORE_LINKS,
  STORE_PREFIXES,
  endstepStatusText,
  firstMatchText,
} from './integrations'

const NOW = 10_000_000_000

function status(connection: ConnectionView, helperMissing = false): EndstepStatusView {
  return { helperMissing, connection, hasAccount: false }
}

describe('browserLabel', () => {
  it('names the known browsers and passes others through', () => {
    expect(browserLabel('chrome')).toBe('Chrome')
    expect(browserLabel('firefox')).toBe('Firefox')
    expect(browserLabel('vivaldi')).toBe('vivaldi')
  })
})

describe('agoText', () => {
  it('says just now, minutes, hours and days', () => {
    expect(agoText(NOW - 10_000, NOW)).toBe('just now')
    expect(agoText(NOW - 5 * 60_000, NOW)).toBe('5 min ago')
    expect(agoText(NOW - 3 * 3_600_000, NOW)).toBe('3 h ago')
    expect(agoText(NOW - 2 * 86_400_000, NOW)).toBe('2 d ago')
    expect(agoText(NOW + 5_000, NOW)).toBe('just now')
  })
})

describe('connectionText', () => {
  it('never seen', () => {
    expect(connectionText(status({ kind: 'neverSeen' }), NOW)).toBe('Not connected yet')
  })

  it('recent shows the browser, version and time', () => {
    const view = status({ kind: 'recent', browser: 'chrome', extension: '0.4.1', atMs: NOW - 120_000 })
    expect(connectionText(view, NOW)).toBe('Connected: Chrome, extension 0.4.1, last seen 2 min ago')
  })

  it('drops a browser or version an older extension does not send', () => {
    const at = NOW - 120_000
    expect(connectionText(status({ kind: 'recent', browser: 'unknown', extension: 'unknown', atMs: at }), NOW)).toBe(
      'Connected, last seen 2 min ago',
    )
    expect(connectionText(status({ kind: 'recent', browser: 'chrome', extension: 'unknown', atMs: at }), NOW)).toBe(
      'Connected: Chrome, last seen 2 min ago',
    )
    expect(connectionText(status({ kind: 'recent', browser: 'unknown', extension: '0.4.1', atMs: at }), NOW)).toBe(
      'Connected: extension 0.4.1, last seen 2 min ago',
    )
  })

  it('stale says when and what to do', () => {
    const view = status({ kind: 'stale', browser: 'firefox', extension: '0.4.1', atMs: NOW - 2 * 3_600_000 })
    expect(connectionText(view, NOW)).toBe(
      'Last seen 2 h ago. Open endstep.cc, or check the extension is switched on in your browser.',
    )
  })

  it('a missing helper wins', () => {
    expect(connectionText(status({ kind: 'neverSeen' }, true), NOW)).toBe(
      'The Endstep helper is missing from this MyMTGO build.',
    )
  })
})

describe('firstMatchText', () => {
  it('shows until an Endstep account exists', () => {
    expect(firstMatchText(status({ kind: 'neverSeen' }))).toBe('Matches appear here after your first Endstep match.')
    expect(firstMatchText({ ...status({ kind: 'neverSeen' }), hasAccount: true })).toBeNull()
  })
})

describe('endstepStatusText', () => {
  it('off says captures wait', () => {
    expect(endstepStatusText(false, status({ kind: 'neverSeen' }), NOW)).toBe(
      'Off. Captures wait in the browser extension until you switch this on.',
    )
  })

  it('on uses the connection text', () => {
    expect(endstepStatusText(true, status({ kind: 'neverSeen' }), NOW)).toBe('Not connected yet')
  })

  it('never says not installed', () => {
    const views = [
      status({ kind: 'neverSeen' }),
      status({ kind: 'neverSeen' }, true),
      status({ kind: 'recent', browser: 'chrome', extension: '1', atMs: NOW }),
      status({ kind: 'stale', browser: 'chrome', extension: '1', atMs: 0 }),
    ]
    const texts = [
      ARENA_ON_TEXT,
      ARENA_OFF_TEXT,
      ...views.flatMap((view) => [endstepStatusText(true, view, NOW), endstepStatusText(false, view, NOW)]),
    ]
    for (const text of texts) expect(text.toLowerCase()).not.toContain('not installed')
  })
})

const CHROME_LISTING = 'https://chromewebstore.google.com/detail/mymtgo-endstep/abc'
const FIREFOX_LISTING = 'https://addons.mozilla.org/firefox/addon/mymtgo-endstep/'

describe('browserInstalls', () => {
  it('a_store_link_replaces_the_manual_steps', () => {
    const [chrome, firefox] = browserInstalls({ chrome: CHROME_LISTING, firefox: FIREFOX_LISTING })
    expect(chrome!.store).toEqual({
      label: 'Add to Chrome',
      hint: 'Also works in Edge and Brave. Then reload any open endstep.cc tab.',
      url: CHROME_LISTING,
    })
    expect(chrome!.steps).toEqual([])
    expect(firefox!.store).toEqual({
      label: 'Add to Firefox',
      hint: 'Then reload any open endstep.cc tab.',
      url: FIREFOX_LISTING,
    })
    expect(firefox!.steps).toEqual([])
  })

  it('every_install_way_says_to_reload_open_endstep_tabs', () => {
    const withStores = browserInstalls({ chrome: CHROME_LISTING, firefox: FIREFOX_LISTING })
    const manual = browserInstalls({ chrome: '', firefox: '' })
    for (const install of withStores) expect(install.store!.hint).toContain('reload any open endstep.cc tab')
    for (const install of manual) expect(install.steps.at(-1)).toBe('Then reload any open endstep.cc tab.')
  })

  it('replaces one browser at a time', () => {
    const [chrome, firefox] = browserInstalls({ chrome: CHROME_LISTING, firefox: '' })
    expect(chrome!.store).not.toBeNull()
    expect(firefox!.store).toBeNull()
    expect(firefox!.steps.join(' ')).toContain('signed build')
  })

  it('an_empty_or_foreign_link_keeps_the_manual_steps', () => {
    const foreign = [
      '',
      'http://chromewebstore.google.com/detail/x',
      'https://example.com/https://chromewebstore.google.com/',
      'https://addons.mozilla.org.evil.test/',
      'javascript:alert(1)',
    ]
    for (const link of foreign) {
      const [chrome, firefox] = browserInstalls({ chrome: link, firefox: link })
      expect(chrome!.store).toBeNull()
      expect(chrome!.steps).toContain('Choose Load unpacked.')
      expect(firefox!.store).toBeNull()
      expect(firefox!.steps).toContain('Open about:debugging.')
    }
  })

  it('does not accept a link for the other browser', () => {
    const [chrome, firefox] = browserInstalls({ chrome: FIREFOX_LISTING, firefox: CHROME_LISTING })
    expect(chrome!.store).toBeNull()
    expect(firefox!.store).toBeNull()
  })
})

describe('STORE_LINKS', () => {
  it('each_link_is_empty_or_a_listing_on_its_own_store', () => {
    for (const store of ['chrome', 'firefox'] as const) {
      const link = STORE_LINKS[store]
      const prefix = STORE_PREFIXES[store]
      expect(
        link === '' || isStoreListing(link, prefix),
        `STORE_LINKS.${store} must be empty or start with ${prefix} (paste the full listing URL, no spaces, https)`,
      ).toBe(true)
    }
  })
})
