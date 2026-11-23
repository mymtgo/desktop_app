// Public half of a throwaway dev key: it only pins the unpacked extension id. The private key was discarded.
export const CHROME_DEV_KEY =
  'MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAs3NNnf6ZNJKvMcURLwJub7qQZWD63FzCqVPOlx2tye1Ay/US2WrGZS8mftrSOiUA23GUuuYFJd06tRzQzna5FYEnpiQWD6bR9FQIwMeTtGgAMmJ3s5XooAqK3QH7zgCneIZ8Tik7L0mbMTW4+vqYKY97Y26FFWwDqeXgZmjyRJR1NegmGfzE3Ywj9RQ53cZmWGXDgDU6NMU7kEohIrWRmLoWaJRvTIdSP+n0ve/KydCqEyksflCFL9awdkdwu2cKAxe2TSrKJywV+IXimGefPSss3x4KvVxEILx6r+mUmUpfQQzx5DvzKUJCycLXIU6u9a36eS0sDIPOQUIgcgLLFwIDAQAB'
export const CHROME_DEV_ID = 'oimkfhogicccpdcbddlfofkmghfhhlgb'

// Copied into dist by scripts/build.mjs from extension/icons.
export const ICONS = {
  '16': 'icons/icon-16.png',
  '32': 'icons/icon-32.png',
  '48': 'icons/icon-48.png',
  '128': 'icons/icon-128.png',
}

export type Browser = 'chrome' | 'firefox'
// dev: sideloaded builds. store: the files uploaded to the Chrome Web Store and Firefox Add-ons.
export type Mode = 'dev' | 'store'

export function manifestFor(browser: Browser, version: string, mode: Mode): object {
  const matches = ['https://endstep.cc/*']
  const base = {
    manifest_version: 3,
    name: 'MyMTGO for Endstep',
    version,
    description: 'Saves your own Endstep matches to the MyMTGO app on your computer.',
    permissions: ['nativeMessaging', 'storage', 'unlimitedStorage', 'alarms', 'scripting'],
    host_permissions: matches,
    content_scripts: [
      { matches, js: ['page-hook.js'], run_at: 'document_start', world: 'MAIN' },
      { matches, js: ['relay.js'], run_at: 'document_start' },
    ],
    icons: ICONS,
    action: { default_popup: 'popup.html', default_icon: ICONS },
  }
  if (browser === 'chrome') {
    // A store upload gets its own id from the Web Store, so the dev key stays out of it.
    const key = mode === 'dev' ? { key: CHROME_DEV_KEY } : {}
    return {
      ...base,
      ...key,
      minimum_chrome_version: '111',
      background: { service_worker: 'background.js' },
    }
  }
  const gecko: Record<string, unknown> = { id: 'endstep@mymtgo.com', strict_min_version: '128.0' }
  if (mode === 'store') {
    // Firefox Add-ons asks every new listing to declare what it collects. Page content goes to the local helper.
    gecko.data_collection_permissions = { required: ['websiteContent'] }
    // Firefox honours data_collection_permissions from version 140, so the store build needs 140.
    gecko.strict_min_version = '140.0'
  }
  return {
    ...base,
    background: { scripts: ['background.js'] },
    browser_specific_settings: { gecko },
  }
}
