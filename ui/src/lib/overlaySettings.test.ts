import { describe, expect, it } from 'vitest'
import type { LeagueDeck, OverlaySettingsView, Settings } from '@/bindings'
import {
  ARTWORK_OPTIONS,
  PREVIEW_OPTIONS,
  PUBLISH_ERRORS,
  firstPreview,
  previewGroups,
  SIZE_OPTIONS,
  WAYLAND_NOTE,
  cardSize,
  cardsAvailability,
  copySizeLabel,
  copySizeText,
  typedFinishedMinutes,
  lastPublishedText,
  obsUrl,
  readOrder,
  sizesUpdating,
  changesSize,
  mergeChange,
  serialSaves,
  partPreview,
  previewCaption,
  sheetPreviewScale,
  sizeHint,
  stripHeight,
  stripResizeNote,
  withStrip,
  previewArt,
  previewState,
  publishErrorText,
  publishHint,
  usesClassicOptions,
  withChange,
  GAMES_OPTIONS,
  SHOW_OPTIONS,
} from './overlaySettings'
import { DEFAULT_DISPLAY, eventText, standingText } from './leagueOverlayCard'

const DECK: LeagueDeck = { id: 7, name: 'Qgreen Tron', archetype: null, colorIdentity: ['G'], label: 'Qlabel' }
const KARN = 'https://cards.scryfall.io/art_crop/karn.jpg'
const BACKGROUND = 'overlay://localhost/background?v=1700'
const NOW = 1_800_000_000_000

function view(overrides: Partial<OverlaySettingsView> = {}): OverlaySettingsView {
  return {
    deck: DECK,
    deckArtUrl: KARN,
    backgroundUrl: null,
    artwork: 'deck',
    publicUrl: 'https://mymtgo.com/overlays/Qstream',
    obsSizes: { full: { width: 332, height: 112 }, compact: { width: 272, height: 90 } },
    publish: { lastPublishedAtMs: null, error: null },
    signedIn: true,
    overlayWindowsAvailable: true,
    ...overrides,
  }
}

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
    overlayEventStrip: true,
    overlayFinishedMinutes: 5,
    overlayLeagueTheme: 'classic',
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

describe('the league window preview', () => {
  it('offers at most eight grouped states per kind (spec 5.5)', () => {
    const labels = (kind: 'league' | 'tournament') =>
      previewGroups(kind).map((group) => [group.label, group.options.map((option) => option.label)])
    expect(labels('league')).toEqual([
      ['Playing', ['In game', 'Sideboarding', 'With a Challenge waiting']],
      ['Between matches', ['Waiting']],
      ['Finished', ['Complete', 'Trophied', 'Dropped', 'Idle']],
    ])
    expect(labels('tournament')).toEqual([
      ['Playing', ['Swiss round', 'Playoff match', 'With a league match live']],
      ['Between matches', ['Registered', 'Waiting for next round']],
      ['Finished', ['Won the event', 'Top 8 finish', 'Missed the cut']],
    ])
    expect(firstPreview('league')).toBe('in_game')
    expect(firstPreview('tournament')).toBe('challenge_round')
  })

  it('previews each option as a card of its kind', () => {
    for (const option of PREVIEW_OPTIONS) {
      const state = previewState(option.value, view(), 'full', DEFAULT_DISPLAY, 'classic', NOW)
      const kind = state.event?.kind ?? 'league'
      expect(kind, option.label).toBe(option.kind)
    }
    expect(previewState('waiting', view(), 'full', DEFAULT_DISPLAY).status).toBe('waiting')
    expect(previewState('league_with_challenge', view(), 'full', DEFAULT_DISPLAY, 'classic', NOW).other?.kind).toBe('tournament')
  })

  it('shows the real last played deck so its label can be edited', () => {
    expect(previewState('in_game', view(), 'full', DEFAULT_DISPLAY).deck).toEqual(DECK)
    expect(previewState('in_game', view({ deck: null }), 'full', DEFAULT_DISPLAY).deck?.name).toBe('tron v3 final')
    expect(previewState('idle', view(), 'full', DEFAULT_DISPLAY).deck).toBeNull()
  })

  it('follows the size', () => {
    expect(previewState('waiting', view(), 'compact', DEFAULT_DISPLAY).size).toBe('compact')
  })

  it('uses the art of the chosen artwork', () => {
    expect(previewArt('deck', BACKGROUND, KARN)).toBe(KARN)
    expect(previewArt('none', BACKGROUND, KARN)).toBeNull()
    expect(previewArt('custom', BACKGROUND, KARN)).toBe(BACKGROUND)
    expect(previewArt('custom', null, KARN)).toBe(KARN)
    expect(previewArt('deck', null, null)).toBeNull()
    expect(previewState('in_game', view({ artwork: 'none' }), 'full', DEFAULT_DISPLAY).art).toBeNull()
    expect(previewState('in_game', view(), 'full', DEFAULT_DISPLAY).art).toEqual({ url: KARN, thumbnail: false })
  })

  it('names the artwork and size choices as 0.x', () => {
    expect(ARTWORK_OPTIONS).toEqual([
      { value: 'deck', label: 'Use deck cover' },
      { value: 'thumbnail', label: 'Side thumbnail' },
      { value: 'none', label: 'No artwork' },
      { value: 'custom', label: 'Custom artwork' },
    ])
    expect(SIZE_OPTIONS).toEqual([
      { value: 'full', label: 'Full' },
      { value: 'compact', label: 'Compact' },
    ])
  })
})

describe('the stream overlay card', () => {
  it('adds ?size=compact to the OBS URL for the compact card', () => {
    expect(obsUrl('https://mymtgo.com/overlays/Qstream', 'full')).toBe('https://mymtgo.com/overlays/Qstream')
    expect(obsUrl('https://mymtgo.com/overlays/Qstream', 'compact')).toBe(
      'https://mymtgo.com/overlays/Qstream?size=compact',
    )
    expect(obsUrl(null, 'compact')).toBeNull()
  })

  it('copies the browser source size', () => {
    const sizes = view().obsSizes
    expect(copySizeText(sizes.full)).toBe('332x112')
    expect(copySizeLabel(sizes.compact, false)).toBe('Copy size (272×90)')
    expect(copySizeLabel(sizes.compact, true)).toBe('Copied')
  })

  it('says when it last published in seconds, then minutes', () => {
    const now = 1_790_000_100_000
    expect(lastPublishedText(null, now)).toBe('Not published yet')
    expect(lastPublishedText(now - 12_000, now)).toBe('Last published 12s ago')
    expect(lastPublishedText(now - 59_000, now)).toBe('Last published 59s ago')
    expect(lastPublishedText(now - 60_000, now)).toBe('Last published 1m ago')
    expect(lastPublishedText(now - 150_000, now)).toBe('Last published 3m ago')
    expect(lastPublishedText(now + 5_000, now)).toBe('Last published 0s ago')
  })

  it('explains every publish error as 0.x', () => {
    expect(PUBLISH_ERRORS).toEqual({
      not_claimed: 'Your MTGO account is not claimed on mymtgo.com yet, so the overlay cannot publish.',
      rejected: 'mymtgo.com refused the overlay data, so the OBS page is not updating. Try a different artwork setting; if that does not help, the website does not take this card yet. The overlay window still works.',
      unreachable: 'Could not reach mymtgo.com. Retrying every 30 seconds.',
      no_login_id: 'Your MTGO account is not identified yet. Play a match, then it will publish.',
    })
    expect(publishErrorText('not_claimed')).toBe(PUBLISH_ERRORS.not_claimed)
    expect(publishErrorText(null)).toBeNull()
  })

  it('disables the publish switch when signed out and says where to sign in', () => {
    expect(cardsAvailability(view({ signedIn: false })).publishDisabled).toBe(true)
    expect(cardsAvailability(view()).publishDisabled).toBe(false)
    expect(publishHint(false)).toBe('Sign in on the Account page to publish.')
    expect(publishHint(true)).toBe('Anyone with the link can see your current event.')
  })
})

describe('native Wayland', () => {
  it('the_window_cards_are_unavailable_on_native_wayland', () => {
    const wayland = cardsAvailability(view({ overlayWindowsAvailable: false }))
    expect(wayland.note).toBe(
      'Overlay windows are not available on Wayland, where they cannot stay above MTGO or remember their position. Start MyMTGO with GDK_BACKEND=x11 to use them.',
    )
    expect(WAYLAND_NOTE).toContain('GDK_BACKEND=x11')
    expect(wayland.windowsDisabled).toBe(true)
    expect(wayland.publishDisabled, 'the OBS card stays available').toBe(false)

    const elsewhere = cardsAvailability(view())
    expect(elsewhere.note).toBeNull()
    expect(elsewhere.windowsDisabled).toBe(false)
  })

  it('saving_on_wayland_never_changes_the_window_settings', () => {
    const stored = settings({ gameOverlay: true, leagueWindow: true, draftNotesWindow: false })
    for (const change of [
      { gameOverlay: false },
      { leagueWindow: false },
      { draftNotesWindow: true },
      { overlayShowOpponent: false },
      { overlayShowDrawOdds: false },
      { overlayShowReveals: false },
      { overlayShowSideboard: false },
    ] satisfies Partial<Settings>[]) {
      expect(withChange(stored, change, false)).toBeNull()
    }
    // The OBS card and the card's look still save, keeping the windows' values.
    expect(withChange(stored, { overlayPublish: true, gameOverlay: false }, false)).toEqual({
      ...stored,
      overlayPublish: true,
    })
    expect(withChange(stored, { overlaySize: 'compact' }, false)).toEqual({ ...stored, overlaySize: 'compact' })
    expect(withChange(stored, { overlayArtwork: 'none' }, false)).toEqual({ ...stored, overlayArtwork: 'none' })
  })

  it('saves the window settings where overlay windows are available', () => {
    const stored = settings()
    expect(withChange(stored, { gameOverlay: true }, true)).toEqual({ ...stored, gameOverlay: true })
    expect(withChange(stored, { overlayShowReveals: false }, true)).toEqual({ ...stored, overlayShowReveals: false })
  })
})

describe('the league card choices', () => {
  it('offer the side thumbnail and both game styles', () => {
    expect(ARTWORK_OPTIONS.map((option) => option.value)).toEqual(['deck', 'thumbnail', 'none', 'custom'])
    expect(GAMES_OPTIONS).toEqual([
      { value: 'dots', label: 'Dots' },
      { value: 'letters', label: 'Letters' },
    ])
    expect(SHOW_OPTIONS.map((option) => option.label)).toEqual([
      'Event name',
      'Format',
      'Archetype',
      'Progress',
      'Status',
      'Record',
      'Standing',
    ])
    expect(SHOW_OPTIONS.filter((option) => option.kind).map((option) => [option.key, option.kind])).toEqual([
      ['standing', 'tournament'],
    ])
  })

  it('preview the chosen display and the thumbnail with the deck art', () => {
    const display = { ...DEFAULT_DISPLAY, record: false }
    const state = previewState('in_game', view({ artwork: 'thumbnail' }), 'full', display)
    expect(state.display).toEqual(display)
    expect(state.art).toEqual({ url: KARN, thumbnail: true })
  })

  it('preview the Format switch doing something', () => {
    const on = eventText(previewState('in_game', view(), 'full', DEFAULT_DISPLAY))
    const off = eventText(previewState('in_game', view(), 'full', { ...DEFAULT_DISPLAY, format: false }))
    expect(on).not.toBe(off)
  })

  it('read the typed finished minutes as a whole number from 1 to 60', () => {
    expect(typedFinishedMinutes('2')).toBe(2)
    expect(typedFinishedMinutes(2)).toBe(2)
    expect(typedFinishedMinutes('1.6')).toBe(2)
    expect(typedFinishedMinutes('0')).toBe(1)
    expect(typedFinishedMinutes('-3')).toBe(1)
    expect(typedFinishedMinutes('600')).toBe(60)
    expect(typedFinishedMinutes('')).toBeNull()
    expect(typedFinishedMinutes(' ')).toBeNull()
    expect(typedFinishedMinutes('soon')).toBeNull()
  })

  it('keep the choices editable where overlay windows cannot open', () => {
    const changed = withChange(settings(), { overlayLeagueDisplay: { ...DEFAULT_DISPLAY, title: false } }, false)
    expect(changed?.overlayLeagueDisplay.title).toBe(false)
  })
})

describe('theme sizes', () => {
  it('copies the Rust size table', () => {
    expect(cardSize('classic', 'full')).toEqual({ width: 300, height: 80 })
    expect(cardSize('classic', 'compact')).toEqual({ width: 240, height: 58 })
    expect(cardSize('versus', 'full')).toEqual({ width: 580, height: 104 })
    expect(cardSize('versus', 'compact')).toEqual({ width: 380, height: 64 })
  })

  it('copies the Rust strip heights: 20 px on Versus Full, else 16', () => {
    expect(stripHeight('classic', 'full')).toBe(16)
    expect(stripHeight('classic', 'compact')).toBe(16)
    expect(stripHeight('versus', 'full')).toBe(20)
    expect(stripHeight('versus', 'compact')).toBe(16)
  })

  it('keeps artwork and games for classic only', () => {
    expect(usesClassicOptions('classic')).toBe(true)
    expect(usesClassicOptions('versus')).toBe(false)
  })
})

describe('the size texts (spec 4.2, 4.3)', () => {
  const WITH_STRIP = { width: 332, height: 128 }

  it('caption the page preview with the look and the OBS size', () => {
    expect(previewCaption('classic', 'full', 'deck', WITH_STRIP)).toBe('Classic · Full · Deck cover · 300 × 80 (OBS 332 × 128)')
    expect(previewCaption('classic', 'compact', 'custom', { width: 272, height: 106 })).toBe(
      'Classic · Compact · Custom artwork · 240 × 58 (OBS 272 × 106)',
    )
    // Artwork applies to Classic only.
    expect(previewCaption('versus', 'full', 'deck', { width: 612, height: 156 })).toBe('Versus · Full · 580 × 104 (OBS 612 × 156)')
  })

  it('hint the card and OBS sizes under Size', () => {
    expect(sizeHint('classic', 'full', WITH_STRIP)).toBe(
      'Card 300 × 80. OBS Browser Source 332 × 128. Changing this changes the OBS size.',
    )
  })

  it('tell the streamer how much taller the source is with the strip', () => {
    expect(stripResizeNote('classic', 'full')).toBe(
      'New in 1.4: the source is 16 px taller to fit the other event strip. Older sizes still work; they just never show the strip.',
    )
    expect(stripResizeNote('versus', 'full')).toContain('20 px taller')
  })
})

describe('the preview and the switches', () => {
  it('drops the strip from the preview while the strip switch is off', () => {
    const state = previewState('league_with_challenge', view(), 'full', DEFAULT_DISPLAY, 'classic', NOW)
    expect(withStrip(state, true).other?.kind).toBe('tournament')
    expect(withStrip(state, false).other).toBeUndefined()
    expect(withStrip(state, false).status).toBe(state.status)
  })

  it('previews a Challenges-only part in a listed Challenge state that shows it', () => {
    expect(partPreview('event')).toBeNull()
    const sample = partPreview('standing')
    expect(sample).not.toBeNull()
    expect(PREVIEW_OPTIONS.find((option) => option.value === sample)?.kind).toBe('tournament')
    for (const theme of ['classic', 'versus'] as const) {
      const state = previewState(sample!, view(), 'full', { ...DEFAULT_DISPLAY, standing: true }, theme, NOW)
      expect(standingText(state), theme).not.toBeNull()
    }
  })

  it('scale the slide-out preview to fit, never up', () => {
    expect(sheetPreviewScale(300, 372)).toBe(1)
    expect(sheetPreviewScale(580, 372)).toBeCloseTo(0.641, 3)
  })
})

describe('saving the slide-out changes one at a time', () => {
  function deferred() {
    let resolve!: (ok: boolean) => void
    const promise = new Promise<boolean>((done) => (resolve = done))
    return { promise, resolve }
  }

  it('merges changes made while a save runs, the latest value per key winning', () => {
    expect(mergeChange(null, { overlaySize: 'compact' })).toEqual({ overlaySize: 'compact' })
    expect(mergeChange({ overlaySize: 'compact', overlayEventStrip: false }, { overlaySize: 'full' })).toEqual({
      overlaySize: 'full',
      overlayEventStrip: false,
    })
  })

  it('saves two quick changes in order, then reads the page once', async () => {
    const saves: Array<{ change: Partial<Settings>; done: ReturnType<typeof deferred> }> = []
    let loads = 0
    const queue = serialSaves(
      (change) => {
        const done = deferred()
        saves.push({ change, done })
        return done.promise
      },
      async () => {
        loads += 1
      },
    )
    const first = queue.change({ overlayLeagueTheme: 'versus' })
    void queue.change({ overlayEventStrip: false })
    void queue.change({ overlaySize: 'compact' })
    expect(saves.map((save) => save.change)).toEqual([{ overlayLeagueTheme: 'versus' }])
    // Shown at once, saved or not.
    expect(queue.unsaved.value).toEqual({ overlayLeagueTheme: 'versus', overlayEventStrip: false, overlaySize: 'compact' })

    saves[0].done.resolve(true)
    await Promise.resolve()
    await Promise.resolve()
    expect(saves.map((save) => save.change)).toEqual([
      { overlayLeagueTheme: 'versus' },
      { overlayEventStrip: false, overlaySize: 'compact' },
    ])
    expect(loads).toBe(0)

    saves[1].done.resolve(true)
    await first
    expect(loads).toBe(1)
    expect(queue.unsaved.value).toBeNull()
  })

  it('knows which changes move the OBS size', () => {
    expect(changesSize(null)).toBe(false)
    expect(changesSize({ overlayFinishedMinutes: 3 })).toBe(false)
    expect(changesSize({ overlayLeagueTheme: 'versus' })).toBe(true)
    expect(changesSize({ overlaySize: 'compact' })).toBe(true)
    expect(changesSize({ overlayEventStrip: false })).toBe(true)
  })

  it('says the OBS size is updating in place of an old one', () => {
    expect(previewCaption('classic', 'full', 'deck', null)).toBe('Classic · Full · Deck cover · 300 × 80 (OBS size updating)')
    expect(sizeHint('classic', 'full', null)).toBe('Card 300 × 80. OBS Browser Source size updating. Changing this changes the OBS size.')
  })

  it('saves the changes after a failed save, and the failed one is no longer shown', async () => {
    const saved: Array<Partial<Settings>> = []
    let loads = 0
    const first = deferred()
    const queue = serialSaves(
      (change) => {
        saved.push(change)
        return saved.length === 1 ? first.promise : Promise.resolve(true)
      },
      async () => {
        loads += 1
      },
    )
    const done = queue.change({ overlaySize: 'compact' })
    void queue.change({ overlayEventStrip: false })
    first.resolve(false)
    await done
    expect(saved).toEqual([{ overlaySize: 'compact' }, { overlayEventStrip: false }])
    expect(loads).toBe(1)
    expect(queue.unsaved.value).toBeNull()
  })

  it('treats a save that throws as failed, and keeps nothing stale for the next change', async () => {
    const saved: Array<Partial<Settings>> = []
    let loads = 0
    const first = deferred()
    const queue = serialSaves(
      (change) => {
        saved.push(change)
        if (saved.length === 1) return first.promise.then(() => Promise.reject(new Error('lost the tracker')))
        return Promise.resolve(true)
      },
      async () => {
        loads += 1
      },
    )
    const done = queue.change({ overlaySize: 'compact' })
    void queue.change({ overlayEventStrip: false })
    first.resolve(true)
    await done
    // The change queued behind the throw still saves, and the page is read again.
    expect(saved).toEqual([{ overlaySize: 'compact' }, { overlayEventStrip: false }])
    expect(loads).toBe(1)
    expect(queue.unsaved.value).toBeNull()

    await queue.change({ overlayFinishedMinutes: 9 })
    expect(saved[2]).toEqual({ overlayFinishedMinutes: 9 })
    expect(loads).toBe(2)
  })

  it('drops waiting changes when the read after a save throws', async () => {
    const saved: Array<Partial<Settings>> = []
    const read = deferred()
    let loads = 0
    const queue = serialSaves(
      async (change) => {
        saved.push(change)
        return true
      },
      async () => {
        loads += 1
        if (loads === 1) {
          await read.promise
          throw new Error('read failed')
        }
      },
    )
    const done = queue.change({ overlayLeagueTheme: 'versus' })
    await Promise.resolve()
    await Promise.resolve()
    void queue.change({ overlaySize: 'compact' })
    read.resolve(true)
    await expect(done).rejects.toThrow('read failed')
    expect(queue.unsaved.value).toBeNull()

    await queue.change({ overlayFinishedMinutes: 9 })
    expect(saved.at(-1)).toEqual({ overlayFinishedMinutes: 9 })
  })

  it('saves a change made while the page is read again, then reads again', async () => {
    const saved: Array<Partial<Settings>> = []
    const reads = [deferred(), deferred()]
    let loads = 0
    const queue = serialSaves(
      async (change) => {
        saved.push(change)
        return true
      },
      async () => {
        await reads[loads++].promise
      },
    )
    const done = queue.change({ overlayLeagueTheme: 'versus' })
    await Promise.resolve()
    await Promise.resolve()
    expect(loads).toBe(1)
    void queue.change({ overlaySize: 'compact' })
    reads[0].resolve(true)
    await Promise.resolve()
    await Promise.resolve()
    expect(saved).toEqual([{ overlayLeagueTheme: 'versus' }, { overlaySize: 'compact' }])
    expect(queue.unsaved.value).toEqual({ overlayLeagueTheme: 'versus', overlaySize: 'compact' })
    reads[1].resolve(true)
    await done
    expect(loads).toBe(2)
    expect(queue.unsaved.value).toBeNull()
  })
})

describe('reading the page while saves run', () => {
  it('keeps the newest read that lands, whichever was asked first', () => {
    const reads = readOrder()
    const afterSave = reads.start()
    const pushed = reads.start()
    // The save's read lands first: kept, though a push asked again since.
    expect(reads.land(afterSave)).toBe(true)
    expect(reads.land(pushed)).toBe(true)
    expect(reads.applied.value).toBe(pushed)

    const older = reads.start()
    const newer = reads.start()
    expect(reads.land(newer)).toBe(true)
    expect(reads.land(older)).toBe(false)
  })

  it('shows the OBS size as updating until a read asked after the save lands', () => {
    const reads = readOrder()
    const before = reads.start()
    // A theme change saved: only a later read carries its size.
    const savedAt = reads.issued()
    expect(sizesUpdating(null, reads.applied.value, savedAt)).toBe(true)
    const after = reads.start()
    expect(reads.land(before)).toBe(true)
    expect(sizesUpdating(null, reads.applied.value, savedAt)).toBe(true)
    expect(reads.land(after)).toBe(true)
    expect(sizesUpdating(null, reads.applied.value, savedAt)).toBe(false)
    // A size change still saving always shows as updating.
    expect(sizesUpdating({ overlaySize: 'compact' }, reads.applied.value, savedAt)).toBe(true)
  })
})
