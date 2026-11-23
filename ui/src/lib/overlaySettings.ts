import type {
  LeagueGames,
  LeagueTheme,
  ObsSize,
  OverlayArtwork,
  OverlaySettingsView,
  OverlaySize,
  PublishError,
  Settings,
} from '@/bindings'
import { shallowRef, type ShallowRef } from 'vue'
import { sampleEventState, type PreviewSample } from '@/components/overlay/overlaySamples'
import type { EventKind, LeagueDisplay, LeagueOverlayState } from '@/bindings'

/**
 * The Overlays settings page's rules (overlay spec 11; 0.x `release_0.46.0`
 * `settings/Overlays.vue` and its four cards). Every string is 0.x's except
 * the OBS card's sign-in hint (v1 has no Sync settings page) and the Wayland
 * note (spec 5.3).
 */

/** The preview state groups (spec 4.2): what the card shows while playing, between matches and once finished. */
export type PreviewGroup = 'playing' | 'between' | 'finished'

export const PREVIEW_GROUP_LABELS: Record<PreviewGroup, string> = {
  playing: 'Playing',
  between: 'Between matches',
  finished: 'Finished',
}

export interface PreviewOption {
  kind: EventKind
  group: PreviewGroup
  value: PreviewSample
  label: string
}

/**
 * The preview states the page offers (spec 5.5): at most eight
 * representative states per kind, for streamers. Every spec 5.4 row has a
 * sample (`TOURNAMENT_ROWS`), but only these are listed.
 */
export const PREVIEW_OPTIONS: PreviewOption[] = [
  { kind: 'league', group: 'playing', value: 'in_game', label: 'In game' },
  { kind: 'league', group: 'playing', value: 'sideboarding', label: 'Sideboarding' },
  { kind: 'league', group: 'playing', value: 'league_with_challenge', label: 'With a Challenge waiting' },
  { kind: 'league', group: 'between', value: 'waiting', label: 'Waiting' },
  { kind: 'league', group: 'finished', value: 'complete', label: 'Complete' },
  { kind: 'league', group: 'finished', value: 'trophied', label: 'Trophied' },
  { kind: 'league', group: 'finished', value: 'dropped', label: 'Dropped' },
  { kind: 'league', group: 'finished', value: 'idle', label: 'Idle' },
  { kind: 'tournament', group: 'playing', value: 'challenge_round', label: 'Swiss round' },
  { kind: 'tournament', group: 'playing', value: 'challenge_playoff', label: 'Playoff match' },
  { kind: 'tournament', group: 'playing', value: 'challenge_with_league', label: 'With a league match live' },
  { kind: 'tournament', group: 'between', value: 'challenge_registered', label: 'Registered' },
  { kind: 'tournament', group: 'between', value: 'challenge_waiting', label: 'Waiting for next round' },
  { kind: 'tournament', group: 'finished', value: 'challenge_won', label: 'Won the event' },
  { kind: 'tournament', group: 'finished', value: 'challenge_top_cut', label: 'Top 8 finish' },
  { kind: 'tournament', group: 'finished', value: 'challenge_missed_cut', label: 'Missed the cut' },
]

/** One kind's preview states in their groups, for a grouped select. */
export function previewGroups(kind: EventKind): Array<{ group: PreviewGroup; label: string; options: PreviewOption[] }> {
  const groups: PreviewGroup[] = ['playing', 'between', 'finished']
  return groups.map((group) => ({
    group,
    label: PREVIEW_GROUP_LABELS[group],
    options: PREVIEW_OPTIONS.filter((option) => option.kind === kind && option.group === group),
  }))
}

/** The state the preview opens on for a kind: its first option. */
export function firstPreview(kind: EventKind): PreviewSample {
  return PREVIEW_OPTIONS.find((option) => option.kind === kind)!.value
}

export const ARTWORK_OPTIONS: Array<{ value: OverlayArtwork; label: string }> = [
  { value: 'deck', label: 'Use deck cover' },
  { value: 'thumbnail', label: 'Side thumbnail' },
  { value: 'none', label: 'No artwork' },
  { value: 'custom', label: 'Custom artwork' },
]

export const SIZE_OPTIONS: Array<{ value: OverlaySize; label: string }> = [
  { value: 'full', label: 'Full' },
  { value: 'compact', label: 'Compact' },
]

export const GAMES_OPTIONS: Array<{ value: LeagueGames; label: string }> = [
  { value: 'dots', label: 'Dots' },
  { value: 'letters', label: 'Letters' },
]

/** The league card's themes, in the order the Theme control lists them. */
export const THEME_OPTIONS: Array<{ value: LeagueTheme; label: string }> = [
  { value: 'classic', label: 'Classic' },
  { value: 'versus', label: 'Versus' },
]

/** Each theme's card size: a copy of `crates/overlay/src/size.rs` (keep both in step). */
export const CARD_SIZES: Record<LeagueTheme, Record<OverlaySize, { width: number; height: number }>> = {
  classic: { full: { width: 300, height: 80 }, compact: { width: 240, height: 58 } },
  versus: { full: { width: 580, height: 104 }, compact: { width: 380, height: 64 } },
}

/** The card's size for a theme, Classic's for a theme this build does not know. */
export function cardSize(theme: LeagueTheme, size: OverlaySize): { width: number; height: number } {
  return (CARD_SIZES[theme] ?? CARD_SIZES.classic)[size]
}

/**
 * The other event strip's height under the card (spec 8.2): a copy of
 * `strip_height` in `crates/overlay/src/size.rs` (keep both in step). The
 * OBS sizes themselves come from Rust (`OverlaySettingsView.obsSizes`).
 */
export function stripHeight(theme: LeagueTheme, size: OverlaySize): number {
  return theme === 'versus' && size === 'full' ? 20 : 16
}

/** What the caption names for each artwork mode. */
const ARTWORK_CAPTIONS: Record<OverlayArtwork, string> = {
  deck: 'Deck cover',
  thumbnail: 'Side thumbnail',
  none: 'No artwork',
  custom: 'Custom artwork',
}

function sizeText(size: { width: number; height: number }): string {
  return `${size.width} × ${size.height}`
}

function labelOf<T extends string>(options: Array<{ value: T; label: string }>, value: T): string {
  return options.find((option) => option.value === value)?.label ?? value
}

/**
 * The page preview's caption (spec 4.2): "Classic · Full · Deck cover ·
 * 300 × 80 (OBS 332 × 128)". Artwork is named for Classic only.
 */
export function previewCaption(
  theme: LeagueTheme,
  size: OverlaySize,
  artwork: OverlayArtwork,
  obs: ObsSize | null,
): string {
  const parts = [labelOf(THEME_OPTIONS, theme), labelOf(SIZE_OPTIONS, size)]
  if (usesClassicOptions(theme)) parts.push(ARTWORK_CAPTIONS[artwork])
  parts.push(`${sizeText(cardSize(theme, size))} (${obs ? `OBS ${sizeText(obs)}` : OBS_UPDATING})`)
  return parts.join(' · ')
}

/** In place of an OBS size while a change to it is still saving: never the old size. */
export const OBS_UPDATING = 'OBS size updating'

/** The hint under Size in the slide-out (spec 4.3); `obs` is `null` while it is updating. */
export function sizeHint(theme: LeagueTheme, size: OverlaySize, obs: ObsSize | null): string {
  const source = obs ? `OBS Browser Source ${sizeText(obs)}` : 'OBS Browser Source size updating'
  return `Card ${sizeText(cardSize(theme, size))}. ${source}. Changing this changes the OBS size.`
}

/** Under the OBS size while the strip switch is on (spec 4.2). */
export function stripResizeNote(theme: LeagueTheme, size: OverlaySize): string {
  return `New in 1.4: the source is ${stripHeight(theme, size)} px taller to fit the other event strip. Older sizes still work; they just never show the strip.`
}

/** The strip switch's sub line (spec 4.3). */
export const STRIP_SWITCH_HINT =
  'When you play two events at once, the other one shows in a strip under the card. Adds 16 px to the OBS Browser Source (20 px on Versus Full).'

/** The preview without the other event strip while its switch is off (the card then never gets `other`). */
export function withStrip(state: LeagueOverlayState, on: boolean): LeagueOverlayState {
  if (on || !state.other) return state
  const { other: _other, ...rest } = state
  return rest
}

/** The slide-out preview's scale: the card fits the available width, never larger than 1:1. */
export function sheetPreviewScale(width: number, available: number): number {
  return Math.min(1, available / width)
}

/** Artwork and the Games style apply to the Classic card only. */
export function usesClassicOptions(theme: LeagueTheme): boolean {
  return theme === 'classic'
}

export type ShowKey = Exclude<keyof LeagueDisplay, 'games'>

/**
 * The card's on/off parts, in the order the settings page lists them. `kind`
 * marks a part that applies to one event kind only (tagged in the UI).
 */
export interface ShowOption {
  key: ShowKey
  label: string
  /** The one-line example under the switch (spec 9). */
  example: string
  kind?: EventKind
  /** A listed preview state that shows the part, for a part of one kind only. */
  preview?: PreviewSample
}

export const SHOW_OPTIONS: ShowOption[] = [
  // What switching each off removes from the event label (spec 5.2): Event
  // name drops a tournament's kind (a league shows none), Format drops the
  // format and leaves the kind ("League", "Challenge").
  { key: 'event', label: 'Event name', example: 'The kind, "Challenge" in "Modern Challenge"' },
  { key: 'format', label: 'Format', example: 'The format, "Modern"' },
  { key: 'title', label: 'Archetype', example: 'Your deck, or the label you typed on the card' },
  { key: 'progress', label: 'Progress', example: 'Match 4/5 in a league; Round 4/6 or Top 8 in a Challenge' },
  { key: 'status', label: 'Status', example: 'Sideboarding, the next round, matches left, results' },
  { key: 'record', label: 'Record', example: '3-1, with draws when there are any' },
  // The rank shows during a Swiss round (and between rounds, a state the list does not offer).
  {
    key: 'standing',
    label: 'Standing',
    example: 'Your rank between rounds, such as 12th of 64',
    kind: 'tournament',
    preview: 'challenge_round',
  },
]

/** The kind tag shown after a one-kind switch's label. */
export const KIND_TAGS: Record<EventKind, string> = { league: 'Leagues', tournament: 'Challenges' }

/**
 * The preview state to switch to when a one-kind part is turned on or off
 * (spec 4.3), or `null` for a part of both kinds.
 */
export function partPreview(key: ShowKey): PreviewSample | null {
  return SHOW_OPTIONS.find((option) => option.key === key)?.preview ?? null
}

/** How long a finished run may stay on the card, in minutes (`overlay_finished_minutes`). */
export const MIN_FINISHED_MINUTES = 1
export const MAX_FINISHED_MINUTES = 60

/**
 * The typed finished minutes as a whole number in range, or `null` when the
 * box is blank or not a number (the stored value then stays).
 */
export function typedFinishedMinutes(typed: string | number): number | null {
  const minutes = Math.round(Number(typed))
  if (String(typed).trim() === '' || !Number.isFinite(minutes)) return null
  return Math.min(MAX_FINISHED_MINUTES, Math.max(MIN_FINISHED_MINUTES, minutes))
}

/** Why the three window cards are unavailable on native Wayland (spec 5.3). */
export const WAYLAND_NOTE =
  'Overlay windows are not available on Wayland, where they cannot stay above MTGO or remember their position. Start MyMTGO with GDK_BACKEND=x11 to use them.'

/** Why OBS may be blank, in words the streamer can act on (0.x). */
export const PUBLISH_ERRORS: Record<PublishError, string> = {
  not_claimed: 'Your MTGO account is not claimed on mymtgo.com yet, so the overlay cannot publish.',
  rejected: 'mymtgo.com refused the overlay data, so the OBS page is not updating. Try a different artwork setting; if that does not help, the website does not take this card yet. The overlay window still works.',
  unreachable: 'Could not reach mymtgo.com. Retrying every 30 seconds.',
  no_login_id: 'Your MTGO account is not identified yet. Play a match, then it will publish.',
}

/** The settings of the overlay windows: unchangeable where they cannot open. */
const WINDOW_KEYS = [
  'leagueWindow',
  'gameOverlay',
  'draftNotesWindow',
  'overlayShowOpponent',
  'overlayShowDrawOdds',
  'overlayShowReveals',
  'overlayShowSideboard',
] as const satisfies ReadonlyArray<keyof Settings>

/** The preview card's art for an artwork mode (0.x `previewArt`). */
export function previewArt(
  artwork: OverlayArtwork,
  backgroundUrl: string | null,
  deckArtUrl: string | null,
): string | null {
  if (artwork === 'none') return null
  if (artwork === 'custom') return backgroundUrl ?? deckArtUrl
  return deckArtUrl
}

/**
 * A preview state with the page's art, the chosen display and the real last
 * played deck. `now` defaults to the current time (the preview's countdowns).
 */
export function previewState(
  sample: PreviewSample,
  view: OverlaySettingsView,
  size: OverlaySize,
  display: LeagueDisplay,
  theme: LeagueTheme = 'classic',
  now?: number,
): LeagueOverlayState {
  return sampleEventState(sample, {
    art: previewArt(view.artwork, view.backgroundUrl, view.deckArtUrl),
    size,
    deck: view.deck,
    display,
    theme,
    thumbnail: view.artwork === 'thumbnail',
    now,
  })
}

/**
 * The hosted page cannot recover on its own when its first load fails (an
 * outage, a blocked network), so OBS should reload it (#506).
 */
export const OBS_REFRESH_TIP =
  'Tip: in the Browser Source, tick "Refresh browser when scene becomes active", so the overlay recovers if it failed to load.'

/** The URL to paste into OBS, carrying the size chosen above. */
export function obsUrl(url: string | null, size: OverlaySize): string | null {
  return url && size === 'compact' ? `${url}?size=compact` : url
}

/** What the "Copy size" button copies. */
export function copySizeText(size: ObsSize): string {
  return `${size.width}x${size.height}`
}

export function copySizeLabel(size: ObsSize, copied: boolean): string {
  return copied ? 'Copied' : `Copy size (${size.width}×${size.height})`
}

/** "Last published {s}s ago", "{m}m ago" or "Not published yet" (0.x). */
export function lastPublishedText(lastPublishedAtMs: number | null, nowMs: number): string {
  if (lastPublishedAtMs === null) return 'Not published yet'
  const seconds = Math.max(0, Math.round((nowMs - lastPublishedAtMs) / 1000))
  return seconds < 60 ? `Last published ${seconds}s ago` : `Last published ${Math.round(seconds / 60)}m ago`
}

export function publishErrorText(error: PublishError | null): string | null {
  return error ? (PUBLISH_ERRORS[error] ?? null) : null
}

export function publishHint(signedIn: boolean): string {
  return signedIn ? 'Anyone with the link can see your current event.' : 'Sign in on the Account page to publish.'
}

/** Which cards can be used: the window cards only where overlay windows open, the OBS card once signed in. */
export function cardsAvailability(view: OverlaySettingsView): {
  note: string | null
  windowsDisabled: boolean
  publishDisabled: boolean
} {
  const windows = view.overlayWindowsAvailable
  return {
    note: windows ? null : WAYLAND_NOTE,
    windowsDisabled: !windows,
    publishDisabled: !view.signedIn,
  }
}

/**
 * The settings to save for a change, or `null` when nothing is left to
 * save. Where overlay windows cannot open, their settings keep their stored
 * values whatever the change says.
 */
export function withChange(stored: Settings, change: Partial<Settings>, windowsAvailable: boolean): Settings | null {
  const kept: Partial<Settings> = { ...change }
  if (!windowsAvailable) {
    for (const key of WINDOW_KEYS) delete kept[key]
  }
  if (Object.keys(kept).length === 0) return null
  return { ...stored, ...kept }
}

/** Two changes as one: the later value wins per key. */
export function mergeChange(earlier: Partial<Settings> | null, later: Partial<Settings>): Partial<Settings> {
  return { ...earlier, ...later }
}

/** Whether a change moves the OBS Browser Source size (theme, size or the strip). */
export function changesSize(change: Partial<Settings> | null): boolean {
  if (!change) return false
  return 'overlayLeagueTheme' in change || 'overlaySize' in change || 'overlayEventStrip' in change
}

/**
 * Saves the page's changes one at a time, so the controls never need to be
 * disabled while a save runs: a change made meanwhile waits, merged with
 * any other waiting change, and is saved right after. Once nothing waits,
 * `done` reads the page again (once). `unsaved` holds every change not yet
 * saved and read back, for the page to show at once.
 */
export function serialSaves(
  save: (change: Partial<Settings>) => Promise<boolean>,
  done: () => Promise<void>,
): { unsaved: ShallowRef<Partial<Settings> | null>; change: (next: Partial<Settings>) => Promise<void> } {
  const unsaved = shallowRef<Partial<Settings> | null>(null)
  let waiting: Partial<Settings> | null = null
  let running = false

  // A save that throws counts as a failed one: the changes behind it still save.
  async function saveOrFail(batch: Partial<Settings>): Promise<boolean> {
    try {
      return await save(batch)
    } catch {
      return false
    }
  }

  async function change(next: Partial<Settings>): Promise<void> {
    waiting = mergeChange(waiting, next)
    unsaved.value = mergeChange(unsaved.value, next)
    if (running) return
    running = true
    try {
      // A change made while the page is read again starts another round.
      while (waiting) {
        while (waiting) {
          const batch: Partial<Settings> = waiting
          waiting = null
          await saveOrFail(batch)
        }
        await done()
      }
    } finally {
      // Nothing outlives a round that stopped early: a later change saves only itself.
      waiting = null
      running = false
      unsaved.value = null
    }
  }

  return { unsaved, change }
}

/**
 * The page's reads in the order they were asked. A read that lands is kept
 * when it is newer than every read kept so far, even if a newer one has
 * been asked since (a push can ask again while a save's read is out).
 */
export function readOrder(): {
  applied: ShallowRef<number>
  issued: () => number
  start: () => number
  land: (read: number) => boolean
} {
  let issued = 0
  const applied = shallowRef(0)
  return {
    applied,
    issued: () => issued,
    start: () => ++issued,
    land: (read) => {
      if (read <= applied.value) return false
      applied.value = read
      return true
    },
  }
}

/**
 * Whether the view's OBS sizes may be old: a theme, size or strip change is
 * still saving, or the read that carries the saved one (any read asked
 * after `savedAtRead`) has not landed yet.
 */
export function sizesUpdating(unsaved: Partial<Settings> | null, appliedRead: number, savedAtRead: number): boolean {
  return changesSize(unsaved) || appliedRead <= savedAtRead
}
