import { describe, expect, it } from 'vitest'
import type {
  AccountStatus,
  ApiStatus,
  DeckSyncRowView,
  OfflineModeState,
  PendingReports,
  SlotCounts,
  SyncStatus,
} from '@/bindings'
import {
  OFFLINE_COOLDOWN_TEXT,
  OFFLINE_COOLDOWN_TITLE,
  accountDot,
  accountStateText,
  archetypeCatalogWarning,
  apiStatusDot,
  apiStatusText,
  apiStatusTone,
  canSignIn,
  cloudText,
  deckListText,
  deckSlotsText,
  deckSyncBlockedReason,
  deckSyncPlatformLabel,
  deckSyncSearchable,
  filterDeckSync,
  formatCounts,
  freePlanText,
  lastErrorText,
  lastSyncedText,
  syncBarText,
  notSyncedText,
  rejectionsText,
  showDeckSlots,
  slotHeldText,
  syncNowLabel,
  uploadCounts,
  offlineLocked,
  pendingMatchesText,
  rejoinBlocked,
  showPendingMatches,
  signInLabel,
  submitPendingLabel,
} from './api'

const ok: ApiStatus = { state: 'ok' }
const offline: ApiStatus = { state: 'offline' }
const noauth: ApiStatus = { state: 'noauth', message: 'Device not recognized.', signedIn: false }
const unreachable: ApiStatus = { state: 'unreachable', error: 'HTTP 502: Bad gateway' }

describe('apiStatusDot', () => {
  it('pulses grey while checking, then green, amber or red (0.x ApiStatusCard)', () => {
    expect(apiStatusDot(null)).toBe('animate-pulse bg-muted-foreground/40')
    expect(apiStatusDot(ok)).toBe('bg-success')
    expect(apiStatusDot(offline)).toBe('bg-warning')
    expect(apiStatusDot(noauth)).toBe('bg-destructive')
    expect(apiStatusDot(unreachable)).toBe('bg-destructive')
  })
})

describe('apiStatusText', () => {
  it('says the state in 0.x words', () => {
    expect(apiStatusText(null)).toBe('Checking…')
    expect(apiStatusText(ok)).toBe('Connected')
    expect(apiStatusText(noauth)).toBe('Device not recognized.')
    expect(apiStatusText(offline)).toBe('Offline mode is enabled')
    expect(apiStatusText(unreachable)).toBe('Cannot reach API')
  })
})

describe('apiStatusTone', () => {
  it('colours the text like the dot, muted while checking or connected', () => {
    expect(apiStatusTone(null)).toBe('text-muted-foreground')
    expect(apiStatusTone(ok)).toBe('text-muted-foreground')
    expect(apiStatusTone(offline)).toBe('text-warning')
    expect(apiStatusTone(noauth)).toBe('text-destructive')
    expect(apiStatusTone(unreachable)).toBe('text-destructive')
  })
})

const signedOut: AccountStatus = {
  configured: true,
  linked: false,
  signingIn: false,
  offline: false,
  error: null,
}
const signedIn: AccountStatus = { ...signedOut, linked: true }

describe('accountStateText and accountDot', () => {
  it('say checking, signed in or not signed in (0.x SyncCard)', () => {
    expect(accountStateText(null)).toBe('Checking...')
    expect(accountStateText(signedIn)).toBe('Signed in')
    expect(accountStateText(signedOut)).toBe('Not signed in')
    expect(accountDot(null)).toBe('animate-pulse bg-muted-foreground/40')
    expect(accountDot(signedIn)).toBe('bg-success')
    expect(accountDot(signedOut)).toBe('bg-muted-foreground/40')
  })
})

describe('signInLabel', () => {
  it('moves from opening the browser to waiting for approval', () => {
    expect(signInLabel(signedOut, false)).toBe('Sign in')
    expect(signInLabel(signedOut, true)).toBe('Opening browser...')
    expect(signInLabel({ ...signedOut, signingIn: true }, false)).toBe('Waiting for approval...')
    expect(signInLabel(null, false)).toBe('Sign in')
  })
})

describe('canSignIn', () => {
  it('needs a configured, online, signed-out client that is not already signing in', () => {
    expect(canSignIn(signedOut, false)).toBe(true)
    expect(canSignIn(null, false)).toBe(false)
    expect(canSignIn(signedOut, true)).toBe(false)
    expect(canSignIn({ ...signedOut, signingIn: true }, false)).toBe(false)
    expect(canSignIn({ ...signedOut, offline: true }, false)).toBe(false)
    expect(canSignIn({ ...signedOut, configured: false }, false)).toBe(false)
    expect(canSignIn(signedIn, false)).toBe(false)
  })
})

describe('offline mode cooldown (0.x OfflineModeCard)', () => {
  const now = 1_800_000_000_000
  const state = (enabled: boolean, lockedUntilMs: number | null): OfflineModeState => ({ enabled, lockedUntilMs })

  it('is locked only while the stored time is in the future', () => {
    expect(offlineLocked(null, now)).toBe(false)
    expect(offlineLocked(now - 1, now)).toBe(false)
    expect(offlineLocked(now, now)).toBe(false)
    expect(offlineLocked(now + 1, now)).toBe(true)
  })

  it('blocks turning offline mode back on only while online and locked', () => {
    expect(rejoinBlocked(null, now)).toBe(false)
    expect(rejoinBlocked(state(false, now + 1), now)).toBe(true)
    expect(rejoinBlocked(state(true, now + 1), now)).toBe(false)
    expect(rejoinBlocked(state(false, now - 1), now)).toBe(false)
    expect(rejoinBlocked(state(false, null), now)).toBe(false)
  })

  it('says when it can come back, in the 0.x words', () => {
    expect(OFFLINE_COOLDOWN_TEXT).toBe(
      'Offline mode is on cooldown after coming back online. You can turn it on again tomorrow.',
    )
    expect(OFFLINE_COOLDOWN_TITLE).toBe('Offline mode is on cooldown until tomorrow after coming back online.')
  })
})

describe('the archetype warning (0.x OfflineModeCard)', () => {
  it('shows only while no downloaded archetype is held, pointing at the download', () => {
    expect(archetypeCatalogWarning(null, false)).toBeNull()
    expect(archetypeCatalogWarning(true, true)).toBeNull()
    expect(archetypeCatalogWarning(false, true)).toBe(
      'No archetype catalog has been downloaded yet, so archetype detection is unreliable. Download archetypes below.',
    )
    expect(archetypeCatalogWarning(false, false)).toBe(
      'No archetype catalog has been downloaded yet. After turning offline mode on, download archetypes to keep detection reliable.',
    )
  })
})

describe('pending match reports (0.x OfflineModeCard)', () => {
  const live: PendingReports = { live: true, count: 2 }

  it('shows only while reports are live and offline mode is off', () => {
    expect(showPendingMatches(null, false)).toBe(false)
    expect(showPendingMatches({ live: false, count: 2 }, false)).toBe(false)
    expect(showPendingMatches(live, true)).toBe(false)
    expect(showPendingMatches(live, false)).toBe(true)
  })

  it('says how many are pending and labels the button in the 0.x words', () => {
    expect(pendingMatchesText(0)).toBe('0 matches pending.')
    expect(pendingMatchesText(1)).toBe('1 matches pending.')
    expect(submitPendingLabel(1, false)).toBe('Submit 1 match')
    expect(submitPendingLabel(3, false)).toBe('Submit 3 matches')
    expect(submitPendingLabel(3, true)).toBe('Submitting...')
  })
})

const syncStatus: SyncStatus = {
  linked: true,
  lastSyncedAtMs: null,
  pending: 3,
  queued: 3,
  pendingByType: { match: 2, deck: 1, league: 0 },
  activity: [],
  syncing: false,
  notSynced: 0,
  slots: null,
  rejections: 0,
  lastError: null,
}

describe('the sync card (0.x SyncCard)', () => {
  it('lists counts per type, plural where needed, or the empty word', () => {
    expect(formatCounts({ match: 1, deck: 2, league: 0 }, 'Nothing')).toBe('1 match, 2 decks')
    expect(formatCounts({ match: 0, deck: 0, league: 3 }, 'Nothing')).toBe('3 leagues')
    expect(formatCounts({ match: 0, deck: 0, league: 0 }, 'Nothing')).toBe('Nothing')
    expect(formatCounts(null, 'Nothing new')).toBe('Nothing new')
  })

  it('leaves decks out of the upload figure: the deck list always syncs', () => {
    expect(uploadCounts(syncStatus)).toEqual({ match: 2, deck: 0, league: 0 })
    expect(uploadCounts(null)).toBeNull()
  })

  it('says what the deck list has to send', () => {
    expect(deckListText(syncStatus)).toBe('1 to send')
    expect(deckListText({ ...syncStatus, pendingByType: { match: 0, deck: 0, league: 0 } })).toBe('Up to date')
  })

  it('says when it last synced', () => {
    expect(lastSyncedText(null)).toBe('Never synced yet')
    expect(lastSyncedText(1_800_000_000_000)).toBe(new Date(1_800_000_000_000).toLocaleString())
  })

  // The status bar: when the last sync ran and how much waits, held matches included.
  it('says when it last synced and what is queued for the status bar', () => {
    const now = new Date(2027, 0, 14, 23, 30).getTime()
    const today = new Date(2027, 0, 14, 23, 0).getTime()
    const earlier = new Date(2027, 0, 12, 9, 5).getTime()
    expect(syncBarText({ ...syncStatus, lastSyncedAtMs: today, queued: 2 }, now)).toBe('Last sync: 23:00 · 2 queued')
    expect(syncBarText({ ...syncStatus, lastSyncedAtMs: today, queued: 0 }, now)).toBe('Last sync: 23:00')
    expect(syncBarText({ ...syncStatus, lastSyncedAtMs: earlier, queued: 1 }, now)).toBe('Last sync: 12 Jan 09:05 · 1 queued')
    expect(syncBarText({ ...syncStatus, lastSyncedAtMs: null, queued: 0 }, now)).toBe('Last sync: never')
    expect(syncBarText({ ...syncStatus, linked: false }, now)).toBeNull()
    expect(syncBarText(undefined, now)).toBeNull()
  })

  it('says what waits in the cloud', () => {
    expect(cloudText(true, null)).toBe('Checking...')
    expect(cloudText(false, null)).toBe('Nothing new')
    expect(cloudText(true, { match: 4, deck: 0, league: 0 })).toBe('4 matches')
  })

  it('shows the deck slots only with a limit', () => {
    const slots: SlotCounts = { limit: 1, used: 1 }
    expect(showDeckSlots(slots)).toBe(true)
    expect(showDeckSlots({ limit: null, used: 2 })).toBe(false)
    expect(showDeckSlots(null)).toBe(false)
    expect(deckSlotsText(slots)).toBe('1 of 1 used')
    expect(deckSlotsText({ limit: null, used: 2 })).toBe('2 in use, no limit')
  })

  it('warns in 0.x words', () => {
    expect(notSyncedText(1)).toBe('1 deleted deck or league is not synced.')
    expect(notSyncedText(2)).toBe('2 deleted decks or leagues are not synced.')
    expect(rejectionsText(1)).toBe('1 row were rejected by the server and are not syncing.')
    expect(rejectionsText(3)).toBe('3 rows were rejected by the server and are not syncing.')
    expect(lastErrorText('HTTP 500')).toBe('The last sync run failed: HTTP 500')
    expect(syncNowLabel(true)).toBe('Syncing...')
    expect(syncNowLabel(false)).toBe('Sync now')
  })
})

const deckRow: DeckSyncRowView = {
  id: 1,
  name: 'Qburn',
  format: 'CMODERN',
  formatLabel: 'Modern',
  archetype: null,
  lastPlayedAtMs: null,
  cloudSyncEnabled: false,
  freesAt: null,
  platform: 'mtgo',
}

describe('the deck sync list (0.x DeckSyncList)', () => {
  it('blocks a fresh deck only while signed out or every slot is in use', () => {
    const full: SlotCounts = { limit: 1, used: 1 }
    expect(deckSyncBlockedReason(deckRow, full, false, false)).toBe('Sign in above to sync decks.')
    expect(deckSyncBlockedReason(deckRow, full, true, false)).toBe(
      'Your slot is in use. Turn another deck off first.',
    )
    expect(deckSyncBlockedReason(deckRow, { limit: 2, used: 2 }, true, false)).toBe(
      'Every slot is in use. Turn another deck off first.',
    )
    expect(deckSyncBlockedReason(deckRow, full, true, true)).toBeNull()
    expect(deckSyncBlockedReason({ ...deckRow, freesAt: '2026-11-05' }, full, true, false)).toBeNull()
    expect(deckSyncBlockedReason(deckRow, { limit: null, used: 9 }, true, false)).toBeNull()
  })

  it('filters by name, format, archetype or platform once the list is long', () => {
    const decks = [deckRow, { ...deckRow, id: 2, name: 'Qelves', formatLabel: 'Pauper', format: 'CPAUPER' }]
    expect(filterDeckSync(decks, ' pauper ').map((deck) => deck.id)).toEqual([2])
    const arena = { ...deckRow, id: 3, platform: 'arena' as const }
    expect(filterDeckSync([...decks, arena], 'arena').map((deck) => deck.id)).toEqual([3])
    expect(filterDeckSync(decks, '').length).toBe(2)
    expect(deckSyncSearchable(new Array(8).fill(deckRow))).toBe(false)
    expect(deckSyncSearchable(new Array(9).fill(deckRow))).toBe(true)
  })

  it('labels each deck with its platform', () => {
    expect(deckSyncPlatformLabel('mtgo')).toBe('MTGO')
    expect(deckSyncPlatformLabel('arena')).toBe('MTG Arena')
    expect(deckSyncPlatformLabel('endstep')).toBe('Endstep')
  })

  it('words the plan and a held slot as 0.x', () => {
    expect(freePlanText(1)).toBe(
      'The free plan syncs 1 constructed deck at a time. Supporter lifts the limit and adds draft and sealed decks.',
    )
    expect(freePlanText(2)).toContain('syncs 2 constructed decks at a time.')
    const day = new Date('2026-11-05T09:30:00Z').toLocaleDateString(undefined, {
      day: 'numeric',
      month: 'short',
      year: 'numeric',
    })
    expect(slotHeldText('2026-11-05T09:30:00Z')).toBe(
      `Slot held until ${day}. Turning this deck back on before then is free.`,
    )
  })
})
