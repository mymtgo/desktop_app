import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { CHROME_DEV_ID, CHROME_DEV_KEY, manifestFor } from './manifest'

describe('manifestFor', () => {
  it('chrome has key, service worker and a MAIN world page hook', () => {
    const m: any = manifestFor('chrome', '1.2.3', 'dev')
    expect(m.manifest_version).toBe(3)
    expect(m.version).toBe('1.2.3')
    expect(m.key).toBe(CHROME_DEV_KEY)
    expect(m.background.service_worker).toBe('background.js')
    const hook = m.content_scripts.find((c: any) => c.js.includes('page-hook.js'))
    expect(hook.world).toBe('MAIN')
    expect(hook.run_at).toBe('document_start')
    expect(hook.matches).toEqual(['https://endstep.cc/*'])
    expect(m.permissions).toContain('nativeMessaging')
  })

  it('firefox has background scripts, gecko id and no key', () => {
    const m: any = manifestFor('firefox', '1.2.3', 'dev')
    expect(m.background.scripts).toEqual(['background.js'])
    expect(m.browser_specific_settings.gecko.id).toBe('endstep@mymtgo.com')
    expect(m.key).toBeUndefined()
    expect(m.permissions).toContain('nativeMessaging')
  })

  it('both builds use the app icon sizes for the extension and the toolbar button', () => {
    for (const browser of ['chrome', 'firefox'] as const) {
      const m: any = manifestFor(browser, '1.2.3', 'dev')
      expect(Object.keys(m.icons)).toEqual(['16', '32', '48', '128'])
      expect(m.action.default_icon).toEqual(m.icons)
      expect(m.icons['32']).toBe('icons/icon-32.png')
    }
  })

  it('both builds ask for the alarms permission for the half hour ping', () => {
    for (const browser of ['chrome', 'firefox'] as const) {
      const m: any = manifestFor(browser, '1.2.3', 'dev')
      expect(m.permissions).toContain('alarms')
    }
  })

  it('the_manifest_asks_for_scripting', () => {
    for (const browser of ['chrome', 'firefox'] as const) {
      const m: any = manifestFor(browser, '1.2.3', 'dev')
      expect(m.permissions).toContain('scripting')
      expect(m.host_permissions).toEqual(['https://endstep.cc/*'])
    }
  })

  it('exposes a 32 letter a-p dev id', () => {
    expect(CHROME_DEV_ID).toMatch(/^[a-p]{32}$/)
  })

  it('a_store_chrome_manifest_has_no_key', () => {
    const m: any = manifestFor('chrome', '1.2.3', 'store')
    expect('key' in m).toBe(false)
  })

  it('a_dev_chrome_manifest_keeps_the_dev_key', () => {
    const m: any = manifestFor('chrome', '1.2.3', 'dev')
    expect(m.key).toBe(CHROME_DEV_KEY)
  })

  it('store_and_dev_manifests_differ_only_where_stated', () => {
    const chromeDev: any = manifestFor('chrome', '1.2.3', 'dev')
    const chromeStore: any = manifestFor('chrome', '1.2.3', 'store')
    delete chromeDev.key
    expect(chromeStore).toEqual(chromeDev)

    const firefoxDev: any = manifestFor('firefox', '1.2.3', 'dev')
    const firefoxStore: any = manifestFor('firefox', '1.2.3', 'store')
    delete firefoxStore.browser_specific_settings.gecko.data_collection_permissions
    firefoxStore.browser_specific_settings.gecko.strict_min_version = firefoxDev.browser_specific_settings.gecko.strict_min_version
    expect(firefoxStore).toEqual(firefoxDev)
  })

  it('the_firefox_store_manifest_declares_its_data_collection', () => {
    const m: any = manifestFor('firefox', '1.2.3', 'store')
    expect(m.browser_specific_settings.gecko.data_collection_permissions).toEqual({ required: ['websiteContent'] })
    const dev: any = manifestFor('firefox', '1.2.3', 'dev')
    expect(dev.browser_specific_settings.gecko.data_collection_permissions).toBeUndefined()
  })

  it('the_firefox_store_build_needs_firefox_140', () => {
    const store: any = manifestFor('firefox', '1.2.3', 'store')
    const dev: any = manifestFor('firefox', '1.2.3', 'dev')
    expect(store.browser_specific_settings.gecko.strict_min_version).toBe('140.0')
    expect(dev.browser_specific_settings.gecko.strict_min_version).toBe('128.0')
  })

  it('the_description_fits_the_chrome_summary_and_is_the_same_in_both_modes', () => {
    for (const browser of ['chrome', 'firefox'] as const) {
      const dev: any = manifestFor(browser, '1.2.3', 'dev')
      const store: any = manifestFor(browser, '1.2.3', 'store')
      expect(dev.description.length).toBeLessThanOrEqual(132)
      expect(store.description).toBe(dev.description)
      expect(dev.description).toBe('Saves your own Endstep matches to the MyMTGO app on your computer.')
    }
  })

  it('store_builds_keep_the_scripting_permission_and_host', () => {
    for (const browser of ['chrome', 'firefox'] as const) {
      const m: any = manifestFor(browser, '1.2.3', 'store')
      expect(m.permissions).toContain('scripting')
      expect(m.host_permissions).toEqual(['https://endstep.cc/*'])
    }
  })

  it('the_version_is_plain_integers', () => {
    const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'))
    expect(pkg.version).toMatch(/^\d+\.\d+\.\d+$/)
  })
})
