import { Archive, CircleUser, Film, HardDrive, Layers, Plug, ShieldCheck, SlidersHorizontal, Wrench } from 'lucide-vue-next'
import type { Component } from 'vue'
import type { Feature } from '@/lib/platformFeatures'

// The settings area in 0.x's layout (release spec section 4; 0.x
// `SettingsSideNav.vue` and `SettingsLayout.vue`), the brand written "MyMTGO"
// (spec ruling 2). Archives, after Storage, is v1's own (league
// self-management spec 5.5).

export type SettingsPageKey = 'general' | 'account' | 'overlays' | 'storage' | 'archives' | 'replays' | 'integrations' | 'privacy' | 'advanced'

export type SettingsNavItem = {
  key: SettingsPageKey
  /** The side nav's label. */
  label: string
  icon: Component
  to: string
  /** The page header's heading and description. */
  title: string
  description: string
  /** Shown only on a platform that has this feature. */
  feature?: Feature
}

export const SETTINGS_NAV_HEADER = {
  title: 'Settings',
  description: 'Configure how MyMTGO tracks and displays your games.',
}

export const SETTINGS_NAV: SettingsNavItem[] = [
  {
    key: 'general',
    label: 'General',
    icon: SlidersHorizontal,
    to: '/settings/general',
    title: 'General',
    description: 'Accounts and how MyMTGO runs on your machine.',
  },
  {
    key: 'account',
    label: 'Account',
    icon: CircleUser,
    to: '/settings/account',
    title: 'Account',
    description: 'Back your matches up to MyMTGO and keep them in sync across your devices.',
  },
  {
    key: 'overlays',
    label: 'Overlays',
    icon: Layers,
    to: '/settings/overlays',
    title: 'Overlays',
    description: 'Always-on-top windows shown during leagues, matches and drafts.',
    feature: 'overlays',
  },
  {
    key: 'storage',
    label: 'Storage',
    icon: HardDrive,
    to: '/settings/storage',
    title: 'Storage',
    description: 'Where MTGO files live and what MyMTGO keeps on disk.',
  },
  {
    key: 'archives',
    label: 'Archives',
    icon: Archive,
    to: '/settings/archives',
    title: 'Archives',
    description: 'Matches and leagues you deleted. Restore puts them back everywhere.',
  },
  {
    key: 'replays',
    label: 'Replays',
    icon: Film,
    to: '/settings/replays',
    title: 'Replays',
    description: 'Record your MTGO matches as replays with the optional recorder.',
  },
  {
    key: 'integrations',
    label: 'Integrations',
    icon: Plug,
    to: '/settings/integrations',
    title: 'Integrations',
    description: 'Switch MTG Arena and Endstep tracking on or off.',
  },
  {
    key: 'privacy',
    label: 'Data & Privacy',
    icon: ShieldCheck,
    to: '/settings/privacy',
    title: 'Data & Privacy',
    description: 'What leaves this device and what comes back in.',
  },
  {
    key: 'advanced',
    label: 'Advanced',
    icon: Wrench,
    to: '/settings/advanced',
    title: 'Advanced',
    description: 'Developer tools and diagnostics.',
  },
]

/** The settings page a path belongs to (its sub-pages included), if any. */
export function settingsPageFor(path: string): SettingsNavItem | null {
  return SETTINGS_NAV.find((item) => path === item.to || path.startsWith(`${item.to}/`)) ?? null
}

/**
 * The Background card's "Closing the window" text: whether a tray exists at
 * runtime (spec ruling 3), then whether close to tray is on (v1's switch; with
 * it off, closing quits as `window::close_action` does).
 */
export function closingWindowText(trayAvailable: boolean, closeToTray: boolean): string {
  if (!trayAvailable) {
    return "Your operating system doesn't support a system tray for this app. Closing the main window will quit MyMTGO and stop match ingestion."
  }
  return closeToTray
    ? 'Closing the main window leaves MyMTGO running in the system tray. Match ingestion continues in the background, so open it again from the tray icon.'
    : 'Closing the main window will quit MyMTGO and stop match ingestion.'
}
