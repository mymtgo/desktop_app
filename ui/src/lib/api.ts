import type {
  AccountStatus,
  ApiStatus,
  DeckSyncRowView,
  OfflineModeState,
  PendingReports,
  PlatformView,
  SlotCounts,
  SyncStatus,
  TypeCounts,
} from '@/bindings'

/** The API Status card's dot: pulsing grey until the first answer, then green, amber (offline) or red. */
export function apiStatusDot(status: ApiStatus | null): string {
  if (status === null) return 'animate-pulse bg-muted-foreground/40'
  switch (status.state) {
    case 'ok':
      return 'bg-success'
    case 'offline':
      return 'bg-warning'
    default:
      return 'bg-destructive'
  }
}

/** The card's line of text (0.x copy, its ellipsis character kept). */
export function apiStatusText(status: ApiStatus | null): string {
  if (status === null) return 'Checking…'
  switch (status.state) {
    case 'ok':
      return 'Connected'
    case 'noauth':
      return status.message
    case 'offline':
      return 'Offline mode is enabled'
    default:
      return 'Cannot reach API'
  }
}

/** The text colour, matching the dot. */
export function apiStatusTone(status: ApiStatus | null): string {
  if (status === null || status.state === 'ok') return 'text-muted-foreground'
  return status.state === 'offline' ? 'text-warning' : 'text-destructive'
}

/** The account card's line (0.x SyncCard). */
export function accountStateText(status: AccountStatus | null): string {
  if (status === null) return 'Checking...'
  return status.linked ? 'Signed in' : 'Not signed in'
}

/** The account card's dot: pulsing grey until the first answer, green once signed in. */
export function accountDot(status: AccountStatus | null): string {
  if (status === null) return 'animate-pulse bg-muted-foreground/40'
  return status.linked ? 'bg-success' : 'bg-muted-foreground/40'
}

/** The sign-in button: opening the browser while the command runs, then waiting for approval. */
export function signInLabel(status: AccountStatus | null, opening: boolean): string {
  if (status?.signingIn) return 'Waiting for approval...'
  if (opening) return 'Opening browser...'
  return 'Sign in'
}

/** Whether "Sign in" can be pressed now. */
export function canSignIn(status: AccountStatus | null, opening: boolean): boolean {
  return status !== null && status.configured && !status.offline && !status.linked && !status.signingIn && !opening
}

/** The offline mode card's cooldown line (0.x OfflineModeCard). */
export const OFFLINE_COOLDOWN_TEXT =
  'Offline mode is on cooldown after coming back online. You can turn it on again tomorrow.'

/** The switch's tooltip during the cooldown (0.x OfflineModeCard). */
export const OFFLINE_COOLDOWN_TITLE = 'Offline mode is on cooldown until tomorrow after coming back online.'

/**
 * The warning while no downloaded archetype is held (0.x OfflineModeCard
 * `hasArchetypeCatalog`), `null` when there is one or it is not known yet.
 * Offline, the card's Download archetypes button fixes it.
 */
export function archetypeCatalogWarning(held: boolean | null, offline: boolean): string | null {
  if (held !== false) return null
  return offline
    ? 'No archetype catalog has been downloaded yet, so archetype detection is unreliable. Download archetypes below.'
    : 'No archetype catalog has been downloaded yet. After turning offline mode on, download archetypes to keep detection reliable.'
}

/** The cooldown after coming back online is still running. */
export function offlineLocked(lockedUntilMs: number | null, nowMs: number): boolean {
  return lockedUntilMs !== null && lockedUntilMs > nowMs
}

/** Offline mode cannot be switched back on yet: online and on cooldown. */
export function rejoinBlocked(state: OfflineModeState | null, nowMs: number): boolean {
  return state !== null && !state.enabled && offlineLocked(state.lockedUntilMs, nowMs)
}

/** The pending match line shows only while match reports are live (ruling 3) and offline mode is off (0.x OfflineModeCard). */
export function showPendingMatches(pending: PendingReports | null, offline: boolean): boolean {
  return pending !== null && pending.live && !offline
}

/** The pending match line (0.x copy). */
export function pendingMatchesText(count: number): string {
  return `${count} matches pending.`
}

/** The submit button (0.x copy). */
export function submitPendingLabel(count: number, submitting: boolean): string {
  if (submitting) return 'Submitting...'
  return `Submit ${count} match${count === 1 ? '' : 'es'}`
}

const PLURALS = { match: 'matches', deck: 'decks', league: 'leagues' } as const

/** "1 match, 2 decks" (0.x SyncCard `formatCounts`); `empty` when nothing counts. */
export function formatCounts(counts: TypeCounts | null | undefined, empty: string): string {
  if (!counts) return empty
  const parts = (['match', 'deck', 'league'] as const)
    .filter((type) => counts[type] > 0)
    .map((type) => `${counts[type]} ${counts[type] === 1 ? type : PLURALS[type]}`)
  return parts.length > 0 ? parts.join(', ') : empty
}

/** "Ready to upload": matches and leagues only, as the deck list always syncs, on every plan. */
export function uploadCounts(status: SyncStatus | null): TypeCounts | null {
  if (status === null) return null
  return { match: status.pendingByType.match, deck: 0, league: status.pendingByType.league }
}

/** The "Deck list" row. */
export function deckListText(status: SyncStatus): string {
  return status.pendingByType.deck > 0 ? `${status.pendingByType.deck} to send` : 'Up to date'
}

/** The "Last synced" row. */
export function lastSyncedText(ms: number | null): string {
  if (ms === null) return 'Never synced yet'
  return new Date(ms).toLocaleString()
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** `HH:MM` today, else `D Mon HH:MM`, in local time. */
function syncClock(ms: number, nowMs: number): string {
  const at = new Date(ms)
  const now = new Date(nowMs)
  const time = `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`
  const sameDay = at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate()
  return sameDay ? time : `${at.getDate()} ${MONTHS[at.getMonth()]} ${time}`
}

/**
 * The status bar's sync line, `Last sync: 23:00 · 2 queued`; null when the
 * device is not linked. `queued` counts the matches still in their 5 minute
 * send hold, so a match that just ended never reads as synced.
 */
export function syncBarText(sync: SyncStatus | null | undefined, nowMs: number): string | null {
  if (!sync?.linked) return null
  const last = `Last sync: ${sync.lastSyncedAtMs === null ? 'never' : syncClock(sync.lastSyncedAtMs, nowMs)}`
  return sync.queued > 0 ? `${last} · ${sync.queued} queued` : last
}

/** The "Waiting in the cloud" row: checking only until the first answer. */
export function cloudText(loading: boolean, download: TypeCounts | null): string {
  if (loading && !download) return 'Checking...'
  return formatCounts(download, 'Nothing new')
}

/** The "Deck slots" row shows only when the plan has a limit. */
export function showDeckSlots(slots: SlotCounts | null): slots is SlotCounts {
  return slots !== null && slots.limit !== null
}

export function deckSlotsText(slots: SlotCounts): string {
  return slots.limit === null ? `${slots.used} in use, no limit` : `${slots.used} of ${slots.limit} used`
}

export function notSyncedText(count: number): string {
  const one = count === 1
  return `${count} deleted deck${one ? '' : 's'} or league${one ? '' : 's'} ${one ? 'is' : 'are'} not synced.`
}

/** 0.x copy, "were" for one row too. */
export function rejectionsText(count: number): string {
  return `${count} row${count === 1 ? '' : 's'} were rejected by the server and are not syncing.`
}

export function lastErrorText(error: string): string {
  return `The last sync run failed: ${error}`
}

export function syncNowLabel(running: boolean): string {
  return running ? 'Syncing...' : 'Sync now'
}

/**
 * Why a deck's switch is off limits (0.x DeckSyncList `blockedReason`): a deck holding a slot can
 * always be turned off, and one cooling down turned back on for free, so only a fresh deck is
 * blocked by a full account.
 */
export function deckSyncBlockedReason(
  deck: DeckSyncRowView,
  slots: SlotCounts,
  linked: boolean,
  on: boolean,
): string | null {
  if (!linked) return 'Sign in above to sync decks.'
  if (on || deck.freesAt !== null) return null
  if (slots.limit !== null && slots.used >= slots.limit) {
    return slots.limit === 1
      ? 'Your slot is in use. Turn another deck off first.'
      : 'Every slot is in use. Turn another deck off first.'
  }
  return null
}

/** The platform label beside a deck in the sync list. */
export function deckSyncPlatformLabel(platform: PlatformView): string {
  if (platform === 'arena') return 'MTG Arena'
  if (platform === 'endstep') return 'Endstep'
  return 'MTGO'
}

/** Long lists get the search box; a handful of decks are quicker to read whole. */
export function deckSyncSearchable(decks: DeckSyncRowView[]): boolean {
  return decks.length > 8
}

export function filterDeckSync(decks: DeckSyncRowView[], search: string): DeckSyncRowView[] {
  const term = search.trim().toLowerCase()
  if (term === '') return decks
  return decks.filter((deck) =>
    [deck.name, deck.formatLabel, deck.archetype ?? '', deckSyncPlatformLabel(deck.platform)].some((field) =>
      field.toLowerCase().includes(term),
    ),
  )
}

export function freePlanText(limit: number): string {
  return `The free plan syncs ${limit} constructed deck${limit === 1 ? '' : 's'} at a time. Supporter lifts the limit and adds draft and sealed decks.`
}

/** 0.x `formatDay`: the server's date as a local day. */
export function formatDay(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })
}

export function slotHeldText(freesAt: string): string {
  return `Slot held until ${formatDay(freesAt)}. Turning this deck back on before then is free.`
}
