import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Settings } from '@/bindings'
import { keys } from '@/lib/keys'

const cache = new Map<string, unknown>()
const commands = {
  saveSettings: vi.fn(),
  chooseLogFolder: vi.fn(),
}
const add = vi.fn()

vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({
    getQueryData: (key: readonly unknown[]) => cache.get(JSON.stringify(key)),
    setQueryData: (key: readonly unknown[], value: unknown) => cache.set(JSON.stringify(key), value),
  }),
}))
vi.mock('@/bindings', () => ({ commands }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add }) }))

const { useSaveSettings } = await import('./useSaveSettings')

function settings(overrides: Partial<Settings> = {}): Settings {
  return {
    logFolder: null,
    closeToTray: true,
    shareUsage: true,
    mtgoDataFolder: null,
    hideArchivedDecks: true,
    decksPerPage: 12,
    interfaceScale: 100,
    deckCardSize: 'large',
    cardImagesPath: null,
    cardStatsTrust: 50,
    gameOverlay: false,
    draftNotesWindow: true,
    leagueWindow: false,
    overlayShowOpponent: true,
    overlayShowDrawOdds: true,
    overlayShowReveals: true,
    overlayShowSideboard: true,
    overlayArtwork: null,
    overlaySize: 'full',
    overlayPublish: false,
    overlayLeagueDisplay: { games: 'dots', event: true, format: true, title: true, progress: true, status: true, record: true, standing: false },
    overlayFinishedMinutes: 5,
    overlayLeagueTheme: 'classic',
    overlayEventStrip: true,
    watcherActive: true,
    activeAccount: null,
    betaUpdates: null,
    recorderEnabled: false,
    recorderAcceptance: null,
    arenaEnabled: null,
    endstepEnabled: false,
    ...overrides,
  }
}

describe('useSaveSettings', () => {
  beforeEach(() => {
    cache.clear()
    commands.saveSettings.mockReset()
    commands.chooseLogFolder.mockReset()
    add.mockReset()
    commands.saveSettings.mockImplementation(async (next: Settings) => ({ status: 'ok', data: next }))
  })

  it('choosing a folder keeps a setting saved while the picker was open', async () => {
    const { save, chooseLogFolder } = useSaveSettings()
    const clicked = settings({ closeToTray: true })
    commands.chooseLogFolder.mockImplementation(async () => {
      await save(settings({ closeToTray: false }))
      return '/logs'
    })

    expect(await chooseLogFolder(clicked)).toBe(true)

    expect(commands.saveSettings).toHaveBeenLastCalledWith(settings({ closeToTray: false, logFolder: '/logs' }))
    expect(cache.get(JSON.stringify(keys.settings))).toEqual(settings({ closeToTray: false, logFolder: '/logs' }))
  })

  it('falls back to the settings it was given when none are cached', async () => {
    const { chooseLogFolder } = useSaveSettings()
    commands.chooseLogFolder.mockResolvedValue('/logs')

    await chooseLogFolder(settings({ closeToTray: false }))

    expect(commands.saveSettings).toHaveBeenLastCalledWith(settings({ closeToTray: false, logFolder: '/logs' }))
  })

  it('a folder picker that fails shows a toast and saves nothing', async () => {
    const { chooseLogFolder } = useSaveSettings()
    commands.chooseLogFolder.mockRejectedValue(new Error('no display'))

    expect(await chooseLogFolder(settings())).toBe(false)

    expect(commands.saveSettings).not.toHaveBeenCalled()
    expect(add).toHaveBeenCalledWith(expect.objectContaining({ type: 'error', message: 'no display' }))
  })

  it('a cancelled picker saves nothing', async () => {
    const { chooseLogFolder } = useSaveSettings()
    commands.chooseLogFolder.mockResolvedValue(null)

    expect(await chooseLogFolder(settings())).toBe(false)
    expect(commands.saveSettings).not.toHaveBeenCalled()
  })
})
