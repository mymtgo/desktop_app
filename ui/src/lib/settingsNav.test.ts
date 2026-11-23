import { Archive, CircleUser, Film, HardDrive, Layers, Plug, ShieldCheck, SlidersHorizontal, Wrench } from 'lucide-vue-next'
import { describe, expect, it } from 'vitest'
import { SETTINGS_NAV, SETTINGS_NAV_HEADER, closingWindowText, settingsPageFor } from './settingsNav'

describe('the settings nav', () => {
  it('has 0.x header with the brand written MyMTGO', () => {
    expect(SETTINGS_NAV_HEADER).toEqual({
      title: 'Settings',
      description: 'Configure how MyMTGO tracks and displays your games.',
    })
  })

  it('lists the pages in 0.x order, Archives after Storage, Replays after Archives, with their icons and routes', () => {
    expect(SETTINGS_NAV.map((item) => [item.label, item.icon, item.to])).toEqual([
      ['General', SlidersHorizontal, '/settings/general'],
      ['Account', CircleUser, '/settings/account'],
      ['Overlays', Layers, '/settings/overlays'],
      ['Storage', HardDrive, '/settings/storage'],
      ['Archives', Archive, '/settings/archives'],
      ['Replays', Film, '/settings/replays'],
      ['Integrations', Plug, '/settings/integrations'],
      ['Data & Privacy', ShieldCheck, '/settings/privacy'],
      ['Advanced', Wrench, '/settings/advanced'],
    ])
  })

  it('gives each page its heading and description', () => {
    expect(SETTINGS_NAV.map((item) => [item.title, item.description])).toEqual([
      ['General', 'Accounts and how MyMTGO runs on your machine.'],
      ['Account', 'Back your matches up to MyMTGO and keep them in sync across your devices.'],
      ['Overlays', 'Always-on-top windows shown during leagues, matches and drafts.'],
      ['Storage', 'Where MTGO files live and what MyMTGO keeps on disk.'],
      ['Archives', 'Matches and leagues you deleted. Restore puts them back everywhere.'],
      ['Replays', 'Record your MTGO matches as replays with the optional recorder.'],
      ['Integrations', 'Switch MTG Arena and Endstep tracking on or off.'],
      ['Data & Privacy', 'What leaves this device and what comes back in.'],
      ['Advanced', 'Developer tools and diagnostics.'],
    ])
  })

  it('finds the active page from the path, sub-pages included', () => {
    expect(settingsPageFor('/settings/general')?.key).toBe('general')
    expect(settingsPageFor('/settings/privacy')?.key).toBe('privacy')
    expect(settingsPageFor('/settings/archives')?.key).toBe('archives')
    expect(settingsPageFor('/settings/advanced/licences')?.key).toBe('advanced')
    expect(settingsPageFor('/settings/generalx')).toBeNull()
    expect(settingsPageFor('/decks')).toBeNull()
  })
})

describe('closingWindowText', () => {
  it('says the app keeps running in the tray when there is one and close to tray is on', () => {
    expect(closingWindowText(true, true)).toBe(
      'Closing the main window leaves MyMTGO running in the system tray. Match ingestion continues in the background, so open it again from the tray icon.',
    )
  })

  it('says closing quits when there is no tray, whatever the switch says', () => {
    for (const closeToTray of [true, false]) {
      expect(closingWindowText(false, closeToTray)).toBe(
        "Your operating system doesn't support a system tray for this app. Closing the main window will quit MyMTGO and stop match ingestion.",
      )
    }
  })

  it('says closing quits when there is a tray but close to tray is off', () => {
    expect(closingWindowText(true, false)).toBe('Closing the main window will quit MyMTGO and stop match ingestion.')
  })
})
