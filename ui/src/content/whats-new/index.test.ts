import { describe, expect, it } from 'vitest'
import { defineComponent } from 'vue'
import { entryFor, majorMinor, whatsNewEntries, whatsNewUrl, whatsNewVersions, type WhatsNewEntry } from './index'

const page = defineComponent({ render: () => null })
const entries: WhatsNewEntry[] = [
  { version: '0.45', component: page },
  { version: '1.0', component: page },
  { version: '1.2', component: page },
]

describe('the What\'s new entries', () => {
  // Release spec section 7: one component per minor version, 1.0 first.
  it('registers the 1.0 page by its major.minor', () => {
    expect(whatsNewVersions(whatsNewEntries)).toContain('1.0')
    expect(whatsNewVersions(entries)).toEqual(['0.45', '1.0', '1.2'])
  })

  it('reads major.minor as the app does (prereleases, a leading v, junk)', () => {
    expect(majorMinor('1.0.0-beta.2')).toEqual([1, 0])
    expect(majorMinor('v1.12.3+build')).toEqual([1, 12])
    expect(majorMinor('junk')).toEqual([0, 0])
  })

  // "The page shows the newest entry not newer than the running version."
  it('picks the newest entry not newer than the running version', () => {
    expect(entryFor('1.0.0-beta.2', entries)?.version).toBe('1.0')
    expect(entryFor('1.1.4', entries)?.version).toBe('1.0')
    expect(entryFor('1.2.0', entries)?.version).toBe('1.2')
    expect(entryFor('9.0.0', entries)?.version).toBe('1.2')
    expect(entryFor('0.44.0', entries)).toBeNull()
    expect(entryFor('1.0.0', [])).toBeNull()
  })

  // WhatsNewPageTest "shares whether the page exists": the links show only then.
  it('links the page only when it exists', () => {
    expect(whatsNewUrl('1.0.0', entries)).toBe('/whats-new')
    expect(whatsNewUrl('0.44.0', entries)).toBeNull()
    expect(whatsNewUrl('1.0.0', [])).toBeNull()
    expect(whatsNewUrl(null, entries)).toBeNull()
  })
})
