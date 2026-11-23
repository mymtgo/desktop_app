import type { EndstepStatusView } from '@/bindings'

// The Integrations settings page's text (spec 2026-11-10 section 8.1).

export const ARENA_ON_TEXT = "Reading MTG Arena's Player.log"
export const ARENA_OFF_TEXT = 'Off. Arena matches stay in MyMTGO and come back when you switch this on.'
export const ENDSTEP_OFF_TEXT = 'Off. Captures wait in the browser extension until you switch this on.'
export const HELPER_MISSING_TEXT = 'The Endstep helper is missing from this MyMTGO build.'
export const FLATPAK_TEXT = 'Flatpak browsers cannot talk to MyMTGO.'

/**
 * The store listing URLs. Owner: after publishing, paste each listing's full
 * URL here and nothing else (Chrome Web Store: `https://chromewebstore.google.com/...`,
 * Firefox Add-ons: `https://addons.mozilla.org/...`). Empty keeps the manual
 * steps. A test fails when a non-empty link does not start with its store's address.
 */
export const STORE_LINKS: StoreLinks = {
  chrome: 'https://chromewebstore.google.com/detail/cbmldoaipgkmodemgdfbajogkdabcopn',
  firefox: 'https://addons.mozilla.org/firefox/addon/mymtgo-for-endstep/',
}

export interface StoreLinks {
  chrome: string
  firefox: string
}

export const STORE_PREFIXES: StoreLinks = {
  chrome: 'https://chromewebstore.google.com/',
  firefox: 'https://addons.mozilla.org/',
}

// The page hook must run before Endstep opens its sockets, so a tab that was
// already open captures nothing until it is reloaded.
const RELOAD_TEXT = 'Then reload any open endstep.cc tab.'

const MANUAL_CHROME_STEPS = [
  'Open the extensions page in your browser.',
  'Switch on Developer mode.',
  'Choose Load unpacked.',
  'Pick the folder MyMTGO testers were given.',
  RELOAD_TEXT,
]
const MANUAL_FIREFOX_STEPS = [
  'Open about:debugging.',
  'Choose This Firefox.',
  'Choose Load Temporary Add-on (a signed build replaces this before beta).',
  RELOAD_TEXT,
]

export interface StoreButton {
  label: string
  hint: string
  url: string
}

/** One browser's install block: a store button when its link is set, otherwise the manual steps. */
export interface BrowserInstall {
  title: string
  store: StoreButton | null
  steps: string[]
}

export function isStoreListing(link: string, prefix: string): boolean {
  return link.startsWith(prefix)
}

/** What to show for each browser, given the store links. A link that is not on its store counts as unset. */
export function browserInstalls(links: StoreLinks): BrowserInstall[] {
  const chromeSet = isStoreListing(links.chrome, STORE_PREFIXES.chrome)
  const firefoxSet = isStoreListing(links.firefox, STORE_PREFIXES.firefox)
  return [
    {
      title: 'Chrome, Edge and Brave',
      store: chromeSet ? { label: 'Add to Chrome', hint: `Also works in Edge and Brave. ${RELOAD_TEXT}`, url: links.chrome } : null,
      steps: chromeSet ? [] : MANUAL_CHROME_STEPS,
    },
    {
      title: 'Firefox',
      store: firefoxSet ? { label: 'Add to Firefox', hint: RELOAD_TEXT, url: links.firefox } : null,
      steps: firefoxSet ? [] : MANUAL_FIREFOX_STEPS,
    },
  ]
}

/** A browser as the extension reports it: `chrome` and `firefox` get a name, others show as sent. */
export function browserLabel(browser: string): string {
  if (browser === 'chrome') return 'Chrome'
  if (browser === 'firefox') return 'Firefox'
  return browser
}

/** How long ago `atMs` was, in the coarsest whole unit. */
export function agoText(atMs: number, nowMs: number): string {
  const minutes = Math.floor((nowMs - atMs) / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  return `${Math.floor(hours / 24)} d ago`
}

/** "Connected" with the browser and version the extension sent, leaving out what an older extension left "unknown". */
function recentText(browser: string, extension: string, ago: string): string {
  const parts: string[] = []
  if (browser !== 'unknown') parts.push(browserLabel(browser))
  if (extension !== 'unknown') parts.push(`extension ${extension}`)
  const seen = `last seen ${ago}`
  return parts.length === 0 ? `Connected, ${seen}` : `Connected: ${parts.join(', ')}, ${seen}`
}

/** The Endstep status line while the switch is on. */
export function connectionText(status: EndstepStatusView, nowMs: number): string {
  if (status.helperMissing) return HELPER_MISSING_TEXT
  const connection = status.connection
  switch (connection.kind) {
    case 'neverSeen':
      return 'Not connected yet'
    case 'recent':
      return recentText(connection.browser, connection.extension, agoText(connection.atMs, nowMs))
    case 'stale':
      return `Last seen ${agoText(connection.atMs, nowMs)}. Open endstep.cc, or check the extension is switched on in your browser.`
  }
}

/** The line under the status while no Endstep match has come in yet; `null` once an Endstep account exists. */
export function firstMatchText(status: EndstepStatusView): string | null {
  return status.hasAccount ? null : 'Matches appear here after your first Endstep match.'
}

/** The Endstep status line for either switch position. */
export function endstepStatusText(on: boolean, status: EndstepStatusView, nowMs: number): string {
  return on ? connectionText(status, nowMs) : ENDSTEP_OFF_TEXT
}
