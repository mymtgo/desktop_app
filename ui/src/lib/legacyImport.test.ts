import { describe, expect, it } from 'vitest'
import type {
  AccountSummary,
  ApiSummary,
  ArchetypeSummary,
  DeckSummary,
  ImportProgress,
  ImportSummary,
  LeagueSummary,
  LegacyImportInfo,
  LimitedSummary,
  OverlaySummary,
} from '@/bindings'
import {
  accountsLine,
  apiLine,
  ARCHETYPE_WAIT_NOTE,
  archetypesLine,
  deckSkipsLine,
  decksLine,
  leagueSkipsLine,
  leaguesLine,
  limitedLine,
  limitedSkipsLine,
  offersFirstImport,
  offersImportHint,
  overlayLine,
  pathLine,
  importRunning,
  progressLine,
  progressPercent,
  replaysLine,
  skipsLine,
  summaryLines,
} from './legacyImport'

const info: LegacyImportInfo = { detectedPath: null, chosenPath: null, lastImport: null, running: false }

const summary: ImportSummary = {
  finishedAtMs: 0,
  path: '/Qdata/nativephp.sqlite',
  added: 5,
  updated: 2,
  unchanged: 1,
  skippedLive: 0,
  skippedInProgress: 0,
  skippedNoLocalPlayer: 0,
  skippedDuplicateToken: 0,
  skippedBadTime: 0,
  skippedBlankToken: 0,
  skippedDuplicateGames: 0,
}

const decks: DeckSummary = {
  tablesFound: true,
  cardsAdded: 6500,
  cardsUpdated: 3,
  cardsUnchanged: 0,
  decksAdded: 30,
  decksUpdated: 1,
  decksUnchanged: 12,
  decksSkippedLimited: 0,
  decksSkippedDuplicate: 0,
  versionsAdded: 80,
  versionsUnchanged: 4,
  versionsSkipped: 0,
  linksAdded: 400,
  linksUnchanged: 2,
}

const leagues: LeagueSummary = {
  tablesFound: true,
  added: 4,
  updated: 1,
  unchanged: 2,
  withoutAccount: 0,
  skippedDeleted: 0,
  skippedUnreadable: 0,
}

describe('pathLine', () => {
  it('says which database is in use', () => {
    expect(pathLine(info)).toBe(
      'No 0.x database found in the usual place. Choose its file (database.sqlite or nativephp.sqlite).',
    )
    expect(pathLine({ ...info, detectedPath: '/Qa.sqlite' })).toBe('Found at /Qa.sqlite')
    expect(pathLine({ ...info, detectedPath: '/Qa.sqlite', chosenPath: '/Qb.sqlite' })).toBe('Using /Qb.sqlite')
  })
})

describe('progressLine', () => {
  it('describes every phase', () => {
    expect(progressLine({ phase: 'reading' })).toBe('Reading 0.x data...')
    expect(progressLine({ phase: 'writing', done: 3, total: 10 })).toBe('Importing 3 of 10 matches')
    expect(progressLine({ phase: 'writing', done: 1, total: 1 })).toBe('Importing 1 of 1 match')
    expect(progressLine({ phase: 'details', done: 40, total: 120 })).toBe('Importing match detail: 40 of 120')
    expect(progressLine({ phase: 'writing', done: 0, total: 0 })).toBe('Importing matches...')
    expect(progressLine({ phase: 'details', done: 0, total: 0 })).toBe('Importing match detail...')
    expect(progressLine({ phase: 'decks' })).toBe('Importing decks...')
    expect(progressLine({ phase: 'leagues' })).toBe('Importing leagues...')
    expect(progressLine({ phase: 'limited' })).toBe('Importing Limited drafts...')
    expect(progressLine({ phase: 'archetypes' })).toBe('Importing archetypes and sideboard guides...')
    expect(progressLine({ phase: 'finished', summary })).toBe('Import finished')
    expect(progressLine({ phase: 'failed', message: 'locked' })).toBe('locked')
  })
})

describe('skipsLine match details', () => {
  it('says how many match details were skipped and to import again', () => {
    expect(skipsLine({ ...summary, skippedDetails: 1 })).toBe('1 match detail skipped (import again to retry)')
    expect(skipsLine({ ...summary, skippedLive: 2, skippedDetails: 3 })).toBe(
      '2 already tracked live, 3 match details skipped (import again to retry)',
    )
    expect(skipsLine({ ...summary, skippedDetails: 0 })).toBeNull()
  })
})

describe('summaryLines', () => {
  it('says when and what the last import did', () => {
    const now = 10 * 60_000
    expect(summaryLines({ ...summary, finishedAtMs: now - 20_000 }, now)).toBe(
      'Last import just now: 5 added, 2 updated, 1 unchanged',
    )
    expect(summaryLines({ ...summary, finishedAtMs: 0 }, now)).toBe(
      'Last import 10 min ago: 5 added, 2 updated, 1 unchanged',
    )
    expect(summaryLines({ ...summary, finishedAtMs: 0 }, 3 * 3_600_000)).toBe(
      'Last import 3 h ago: 5 added, 2 updated, 1 unchanged',
    )
    expect(summaryLines({ ...summary, finishedAtMs: 0 }, 2 * 86_400_000)).toBe(
      'Last import 2 days ago: 5 added, 2 updated, 1 unchanged',
    )
  })
})

describe('skipsLine', () => {
  it('omits zero skips and is null when nothing was skipped', () => {
    expect(skipsLine(summary)).toBeNull()
    expect(skipsLine({ ...summary, skippedLive: 4, skippedInProgress: 1 })).toBe(
      '4 already tracked live, 1 still in progress in 0.x',
    )
    expect(
      skipsLine({ ...summary, skippedNoLocalPlayer: 2, skippedDuplicateToken: 3, skippedBadTime: 1 }),
    ).toBe('2 without a local player, 3 duplicates, 1 with a bad start time')
    expect(skipsLine({ ...summary, skippedBlankToken: 2, skippedDuplicateGames: 1 })).toBe(
      '2 without a match token, 1 repeated game',
    )
    expect(skipsLine({ ...summary, skippedDuplicateGames: 3 })).toBe('3 repeated games')
  })

  it('counts the matches of players that were not 0.x accounts (#337)', () => {
    expect(skipsLine({ ...summary, skippedNotAccount: 1 })).toBe(
      '1 from a player that was not one of your 0.x accounts',
    )
    expect(skipsLine({ ...summary, skippedInProgress: 1, skippedNotAccount: 3 })).toBe(
      '1 still in progress in 0.x, 3 from players that were not your 0.x accounts',
    )
  })
})

describe('decksLine', () => {
  it('counts decks, versions, cards and match links', () => {
    expect(decksLine({ ...summary, decks })).toBe(
      'Decks: 30 added, 1 updated, 12 unchanged. Versions: 80 added. Cards: 6500 added, 3 updated. Match links: 400 added.',
    )
  })

  it('says when the 0.x database has no decks', () => {
    expect(decksLine({ ...summary, decks: { ...decks, tablesFound: false } })).toBe(
      'This 0.x database has no decks, so only matches were imported.',
    )
  })

  it('is null for a summary stored before decks were imported', () => {
    expect(decksLine(summary)).toBeNull()
    expect(decksLine({ ...summary, decks: null })).toBeNull()
  })
})

describe('deckSkipsLine', () => {
  it('lists what was not imported, or null', () => {
    expect(deckSkipsLine(summary)).toBeNull()
    expect(deckSkipsLine({ ...summary, decks })).toBeNull()
    expect(
      deckSkipsLine({ ...summary, decks: { ...decks, decksSkippedLimited: 3, versionsSkipped: 1 }, limited }),
    ).toBe('Not imported: 1 old deck version that could not be read')
    // Limited decks come over as the registered decks of their matches.
    expect(deckSkipsLine({ ...summary, decks: { ...decks, decksSkippedLimited: 1 }, limited })).toBeNull()
    expect(deckSkipsLine({ ...summary, decks: { ...decks, versionsSkipped: 2 } })).toBe(
      'Not imported: 2 old deck versions that could not be read',
    )
    // Issue #122 M13: duplicate 0.x deck ids are counted.
    expect(deckSkipsLine({ ...summary, decks: { ...decks, decksSkippedDuplicate: 1, versionsSkipped: 2 } })).toBe(
      "Not imported: 1 deck sharing another deck's MTGO id, 2 old deck versions that could not be read",
    )
  })
})

describe('deckSkipsLine for a summary stored before Limited was imported', () => {
  // Issue #194: that import left the Limited decks out, so its line stays.
  it('still lists the Limited decks left out', () => {
    expect(deckSkipsLine({ ...summary, decks: { ...decks, decksSkippedLimited: 3, versionsSkipped: 1 } })).toBe(
      'Not imported: 3 Limited decks, 1 old deck version that could not be read',
    )
    expect(deckSkipsLine({ ...summary, decks: { ...decks, decksSkippedLimited: 1 }, limited: null })).toBe(
      'Not imported: 1 Limited deck',
    )
  })
})

describe('replaysLine', () => {
  it('is null when 0.x had no shared replay links, or for an older summary', () => {
    expect(replaysLine(summary)).toBeNull()
    expect(replaysLine({ ...summary, replaySharesAdded: 0, replaySharesIncomplete: 0, legacyGameLinks: 0 })).toBeNull()
  })

  it('counts the links and says where to switch off the ones left behind', () => {
    expect(replaysLine({ ...summary, replaySharesAdded: 2 })).toBe('Replays: 2 shared replay links brought over.')
    expect(
      replaysLine({
        ...summary,
        replaySharesAdded: 1,
        replaySharesIncomplete: 1,
        replaySharesUnplaced: 1,
        legacyGameLinks: 2,
      }),
    ).toBe(
      'Replays: 1 shared replay link brought over. 1 incomplete link was left out. ' +
        '3 replay links stay shared on mymtgo.com but are not linked to a match here ' +
        '(1 whose match was not imported, is tracked here under another ID or already has a link; ' +
        '2 per-game links from a pre-release build): ' +
        'switch them off on your Account page on mymtgo.com.',
    )
  })

  // #307 M4: a rerun brings nothing over, so it does not say "0 brought over".
  it('leaves out the brought over sentence when none came over', () => {
    expect(replaysLine({ ...summary, legacyGameLinks: 1 })).toBe(
      'Replays: 1 replay link stays shared on mymtgo.com but is not linked to a match here ' +
        '(1 per-game link from a pre-release build): switch it off on your Account page on mymtgo.com.',
    )
    // #307: these links' matches were left out, tracked here under another
    // ID, or already hold a link; all three stay shared on mymtgo.com.
    expect(replaysLine({ ...summary, replaySharesUnplaced: 2 })).toBe(
      'Replays: 2 replay links stay shared on mymtgo.com but are not linked to a match here ' +
        '(2 whose matches were not imported, are tracked here under another ID or already have a link): ' +
        'switch them off on your Account page on mymtgo.com.',
    )
  })
})

describe('leagueSkipsLine for a summary stored before Limited was imported', () => {
  // Issue #194 (review M5): that import left draft and sealed leagues out.
  it('still lists the Limited leagues left out', () => {
    expect(leagueSkipsLine({ ...summary, leagues: { ...leagues, skippedLimited: 2 } })).toBe(
      'Leagues not imported: 2 Limited leagues',
    )
    expect(leagueSkipsLine({ ...summary, leagues: { ...leagues, skippedLimited: 2 }, limited })).toBeNull()
  })
})

describe('leaguesLine', () => {
  it('counts the leagues', () => {
    expect(leaguesLine({ ...summary, leagues })).toBe('Leagues: 4 added, 1 updated, 2 unchanged.')
  })

  it('says how many leagues found no account', () => {
    expect(leaguesLine({ ...summary, leagues: { ...leagues, withoutAccount: 1 } })).toBe(
      'Leagues: 4 added, 1 updated, 2 unchanged. 1 league could not be matched to an account.',
    )
    expect(leaguesLine({ ...summary, leagues: { ...leagues, withoutAccount: 2 } })).toBe(
      'Leagues: 4 added, 1 updated, 2 unchanged. 2 leagues could not be matched to an account.',
    )
  })

  it('is null without a leagues table or for a summary stored before leagues were imported', () => {
    expect(leaguesLine({ ...summary, leagues: { ...leagues, tablesFound: false } })).toBeNull()
    expect(leaguesLine(summary)).toBeNull()
    expect(leaguesLine({ ...summary, leagues: null })).toBeNull()
  })
})

describe('leagueSkipsLine', () => {
  it('lists the leagues not imported, or null', () => {
    expect(leagueSkipsLine(summary)).toBeNull()
    expect(leagueSkipsLine({ ...summary, leagues })).toBeNull()
    expect(
      leagueSkipsLine({
        ...summary,
        leagues: { ...leagues, skippedDeleted: 1, skippedUnreadable: 3 },
      }),
    ).toBe('Leagues not imported: 1 deleted league, 3 unreadable leagues')
    expect(leagueSkipsLine({ ...summary, leagues: { ...leagues, skippedUnreadable: 1 } })).toBe(
      'Leagues not imported: 1 unreadable league',
    )
    expect(leagueSkipsLine({ ...summary, leagues: { ...leagues, skippedNotAccount: 2 } })).toBe(
      'Leagues not imported: 2 leagues of players that were not your 0.x accounts',
    )
  })

  it('reads a summary stored after Limited leagues were imported without a Limited line', () => {
    const counted = { ...leagues, skippedLimited: 2 }
    expect(leagueSkipsLine({ ...summary, leagues: counted, limited })).toBeNull()
  })
})

const limited: LimitedSummary = {
  tablesFound: true,
  draftsAdded: 1,
  draftsUpdated: 0,
  draftsUnchanged: 2,
  draftsWithoutAccount: 0,
  draftsSkipped: 0,
  notesAdded: 3,
  notesSkipped: 0,
  decksAdded: 5,
  decksUpdated: 0,
  decksUnchanged: 1,
  decksWithoutRun: 0,
  decksSkipped: 0,
}

describe('limitedLine', () => {
  it('counts the drafts, notes and registered decks', () => {
    expect(limitedLine({ ...summary, limited })).toBe(
      'Limited: 1 draft added, 0 updated, 2 unchanged. 3 pick notes added. Registered decks: 5 added, 0 updated, 1 unchanged.',
    )
    expect(limitedLine({ ...summary, limited: { ...limited, draftsAdded: 2, notesAdded: 1 } })).toBe(
      'Limited: 2 drafts added, 0 updated, 2 unchanged. 1 pick note added. Registered decks: 5 added, 0 updated, 1 unchanged.',
    )
  })

  it('says how many drafts found no account', () => {
    expect(limitedLine({ ...summary, limited: { ...limited, draftsWithoutAccount: 1 } })).toBe(
      'Limited: 1 draft added, 0 updated, 2 unchanged. 3 pick notes added. Registered decks: 5 added, 0 updated, 1 unchanged. 1 draft could not be matched to an account.',
    )
  })

  it('is null without Limited tables or for a summary stored before Limited was imported', () => {
    expect(limitedLine({ ...summary, limited: { ...limited, tablesFound: false } })).toBeNull()
    expect(limitedLine(summary)).toBeNull()
    expect(limitedLine({ ...summary, limited: null })).toBeNull()
  })
})

describe('limitedSkipsLine', () => {
  it('lists the Limited data not imported, or null', () => {
    expect(limitedSkipsLine(summary)).toBeNull()
    expect(limitedSkipsLine({ ...summary, limited })).toBeNull()
    expect(
      limitedSkipsLine({
        ...summary,
        limited: { ...limited, draftsSkipped: 1, decksWithoutRun: 2, decksSkipped: 3 },
      }),
    ).toBe(
      'Limited not imported: 1 unreadable draft, 2 registered decks of leagues not imported, 3 unreadable registered decks',
    )
    expect(limitedSkipsLine({ ...summary, limited: { ...limited, decksWithoutRun: 1 } })).toBe(
      'Limited not imported: 1 registered deck of a league not imported',
    )
    expect(limitedSkipsLine({ ...summary, limited: { ...limited, draftsNotAccount: 1 } })).toBe(
      'Limited not imported: 1 draft of a player that was not one of your 0.x accounts',
    )
    expect(limitedSkipsLine({ ...summary, limited: { ...limited, notesSkipped: 2 } })).toBe(
      'Limited not imported: 2 pick notes of picks not read from the log yet (import again later)',
    )
  })
})

const accounts: AccountSummary = {
  added: 2,
  unchanged: 1,
  loginIdsFilled: 0,
  loginIdClashes: 0,
  untracked: 0,
  activeSeeded: false,
}

describe('accountsLine', () => {
  it('counts the accounts', () => {
    expect(accountsLine({ ...summary, accounts })).toBe('Accounts: 2 added, 1 unchanged.')
  })

  it('adds the login ids filled and the clashes', () => {
    expect(accountsLine({ ...summary, accounts: { ...accounts, loginIdsFilled: 1, loginIdClashes: 2 } })).toBe(
      'Accounts: 2 added, 1 unchanged. 1 login id filled. 2 login ids already belong to another account.',
    )
    expect(accountsLine({ ...summary, accounts: { ...accounts, loginIdsFilled: 3, loginIdClashes: 1 } })).toBe(
      'Accounts: 2 added, 1 unchanged. 3 login ids filled. 1 login id already belongs to another account.',
    )
  })

  // Issue #310: tracking and the current account carried from 0.x.
  it('says which accounts arrived untracked and that the current account came from 0.x', () => {
    expect(accountsLine({ ...summary, accounts: { ...accounts, untracked: 1, activeSeeded: true } })).toBe(
      'Accounts: 2 added, 1 unchanged. 1 account not recording matches, as in 0.x. Current account set from 0.x.',
    )
    expect(accountsLine({ ...summary, accounts: { ...accounts, untracked: 2 } })).toBe(
      'Accounts: 2 added, 1 unchanged. 2 accounts not recording matches, as in 0.x.',
    )
  })

  it('is null when the 0.x database had no accounts to import', () => {
    const none = { ...accounts, added: 0, unchanged: 0, loginIdsFilled: 0, loginIdClashes: 0 }
    expect(accountsLine({ ...summary, accounts: none })).toBeNull()
  })

  it('is null for a summary stored before accounts were imported', () => {
    expect(accountsLine(summary)).toBeNull()
    expect(accountsLine({ ...summary, accounts: null })).toBeNull()
  })
})

describe('offersFirstImport', () => {
  it('offers the import when a 0.x database was found and nothing was imported yet', () => {
    expect(offersFirstImport({ ...info, detectedPath: '/Qdata/nativephp.sqlite' })).toBe(true)
  })

  it('stays quiet without a found database, after an import, or before the info loads', () => {
    expect(offersFirstImport(info)).toBe(false)
    expect(offersFirstImport({ ...info, chosenPath: '/Qchosen/nativephp.sqlite' })).toBe(false)
    expect(offersFirstImport({ ...info, detectedPath: '/Qdata/nativephp.sqlite', lastImport: summary })).toBe(false)
    expect(offersFirstImport(undefined)).toBe(false)
  })
})

describe('importRunning', () => {
  it('is running in every phase before the end, the match detail step included', () => {
    const phases: ImportProgress[] = [
      { phase: 'reading' },
      { phase: 'writing', done: 1, total: 2 },
      { phase: 'details', done: 1, total: 2 },
      { phase: 'decks' },
      { phase: 'leagues' },
    ]
    for (const progress of phases) expect(importRunning(progress, false)).toBe(true)
  })

  it('is not running once the import finished or failed, whatever the stored info says', () => {
    expect(importRunning({ phase: 'failed', message: 'x' }, true)).toBe(false)
  })

  it('falls back to the stored info when no progress was seen', () => {
    expect(importRunning(null, true)).toBe(true)
    expect(importRunning(undefined, false)).toBe(false)
  })
})

describe('progressPercent', () => {
  it('fills the bar while writing matches and while importing match detail', () => {
    expect(progressPercent({ phase: 'writing', done: 1, total: 4 })).toBe(25)
    expect(progressPercent({ phase: 'details', done: 3, total: 4 })).toBe(75)
  })

  it('has no bar in the other phases or with nothing to do', () => {
    expect(progressPercent({ phase: 'decks' })).toBeNull()
    expect(progressPercent({ phase: 'details', done: 0, total: 0 })).toBeNull()
  })
})

const api: ApiSummary = {
  reportsSent: 2,
  cardStatsAdded: 1,
  cardStatsSkipped: 0,
  syncedRows: 4,
  deckFlags: 1,
  syncStates: 3,
  rejections: 0,
  tournamentsNamed: 1,
  tournamentTokens: 2,
}

describe('apiLine', () => {
  it('counts what 0.x reported and synced', () => {
    expect(apiLine({ ...summary, api })).toBe(
      'MyMTGO: 2 match reports already sent, 1 card stats game queued, 4 synced rows and 1 tournament name brought over.',
    )
    expect(apiLine({ ...summary, api: { ...api, reportsSent: 1, cardStatsAdded: 2, syncedRows: 1, tournamentsNamed: 0 } })).toBe(
      'MyMTGO: 1 match report already sent, 2 card stats games queued, 1 synced row and 0 tournament names brought over.',
    )
  })

  it('says how many card stats games had no match', () => {
    expect(apiLine({ ...summary, api: { ...api, cardStatsSkipped: 3 } })).toBe(
      'MyMTGO: 2 match reports already sent, 1 card stats game queued, 4 synced rows and 1 tournament name brought over. 3 card stats games of matches not imported were left out.',
    )
  })

  it('is null for a summary stored before the API state was imported', () => {
    expect(apiLine(summary)).toBeNull()
    expect(apiLine({ ...summary, api: null })).toBeNull()
  })
})

const archetypes: ArchetypeSummary = {
  archetypesAdded: 9,
  archetypesUpdated: 0,
  archetypesUnchanged: 1,
  matchRowsAdded: 7,
  matchRowsKept: 0,
  matchRowsSkipped: 0,
  matchesDetected: 3,
  decksLinked: 2,
  guidesAdded: 3,
  guidesKept: 0,
  guideCardsAdded: 2,
  notesAdded: 3,
  notesUnchanged: 0,
  notesSkipped: 0,
}

describe('archetypesLine', () => {
  it('counts the archetypes, match and deck archetypes, guides and notes', () => {
    expect(archetypesLine({ ...summary, archetypes })).toBe(
      'Archetypes: 9 added, 0 updated, 1 unchanged. 7 match archetypes, 2 deck archetypes, 3 sideboard guides and 3 matchup notes brought over.',
    )
    expect(
      archetypesLine({
        ...summary,
        archetypes: { ...archetypes, matchRowsAdded: 1, decksLinked: 1, guidesAdded: 1, notesAdded: 1 },
      }),
    ).toBe(
      'Archetypes: 9 added, 0 updated, 1 unchanged. 1 match archetype, 1 deck archetype, 1 sideboard guide and 1 matchup note brought over.',
    )
  })

  // #300 F15: plain wording for what was left out, and the guides and
  // notes v1 kept or could not take.
  it('says how many match archetypes were left out and why', () => {
    expect(archetypesLine({ ...summary, archetypes: { ...archetypes, matchRowsSkipped: 2 } })).toBe(
      'Archetypes: 9 added, 0 updated, 1 unchanged. 7 match archetypes, 2 deck archetypes, 3 sideboard guides and 3 matchup notes brought over. 2 match archetypes were left out: their match, player or archetype is not in v1.',
    )
    expect(archetypesLine({ ...summary, archetypes: { ...archetypes, matchRowsSkipped: 1 } })).toContain(
      '1 match archetype was left out: its match, player or archetype is not in v1.',
    )
  })

  it('says how many guides v1 kept and how many notes were left out', () => {
    expect(archetypesLine({ ...summary, archetypes: { ...archetypes, guidesKept: 2, notesSkipped: 1 } })).toBe(
      'Archetypes: 9 added, 0 updated, 1 unchanged. 7 match archetypes, 2 deck archetypes, 3 sideboard guides and 3 matchup notes brought over. 2 sideboard guides v1 already had were kept. 1 matchup note was left out: its deck or archetype is not in v1, or it is empty or too long.',
    )
  })

  it('tells that imported matches wait for a finished import', () => {
    expect(ARCHETYPE_WAIT_NOTE).toBe(
      'Imported matches get their archetypes after the import finishes; if reading 0.x fails, they wait for a successful import.',
    )
  })

  it('says how many match archetypes v1 kept as they were', () => {
    expect(archetypesLine({ ...summary, archetypes: { ...archetypes, matchRowsKept: 1 } })).toBe(
      'Archetypes: 9 added, 0 updated, 1 unchanged. 7 match archetypes, 2 deck archetypes, 3 sideboard guides and 3 matchup notes brought over. 1 match archetype already set in v1 was kept.',
    )
    expect(archetypesLine({ ...summary, archetypes: { ...archetypes, matchRowsKept: 3 } })).toContain(
      '3 match archetypes already set in v1 were kept.',
    )
  })

  it('is null for a summary stored before archetypes were imported', () => {
    expect(archetypesLine(summary)).toBeNull()
    expect(archetypesLine({ ...summary, archetypes: null })).toBeNull()
  })
})

const overlay: OverlaySummary = {
  labelsAdded: 2,
  labelsKept: 0,
  labelsUnknown: 0,
  background: 'not_set',
}

describe('overlayLine', () => {
  it('counts the deck labels brought over', () => {
    expect(overlayLine({ ...summary, overlay })).toBe('Overlay: 2 deck labels brought over.')
    expect(overlayLine({ ...summary, overlay: { ...overlay, labelsAdded: 1 } })).toBe(
      'Overlay: 1 deck label brought over.',
    )
  })

  it('says which labels stayed and which found no deck', () => {
    expect(overlayLine({ ...summary, overlay: { ...overlay, labelsKept: 1, labelsUnknown: 3 } })).toBe(
      'Overlay: 2 deck labels brought over. 1 deck label already set in v1 was kept. ' +
        '3 deck labels of decks not imported were left out.',
    )
  })

  it('says what happened to the custom background', () => {
    const line = (background: OverlaySummary['background']) =>
      overlayLine({ ...summary, overlay: { ...overlay, background } })
    expect(line('copied')).toBe('Overlay: 2 deck labels brought over. The custom league card background was copied.')
    expect(line('kept')).toBe('Overlay: 2 deck labels brought over. v1\'s own league card background was kept.')
    expect(line('missing')).toBe(
      'Overlay: 2 deck labels brought over. The custom league card background file was not found in 0.x, so it was skipped.',
    )
    expect(line('failed')).toBe(
      'Overlay: 2 deck labels brought over. The custom league card background could not be copied from 0.x, so it was skipped.',
    )
    expect(line('refused')).toBe(
      'Overlay: 2 deck labels brought over. The custom league card background is not a jpeg, png, webp or gif of at most 5 MB, so it was skipped.',
    )
  })

  it('is null for a summary stored before the overlay was imported', () => {
    expect(overlayLine(summary)).toBeNull()
    expect(overlayLine({ ...summary, overlay: null })).toBeNull()
  })
})

describe('offersImportHint', () => {
  it('shows for a new user with nothing detected', () => {
    expect(offersImportHint(info, false)).toBe(true)
  })

  it('stays hidden while loading, with matches, or once a database or import exists', () => {
    expect(offersImportHint(undefined, false)).toBe(false)
    expect(offersImportHint(info, true)).toBe(false)
    expect(offersImportHint({ ...info, detectedPath: '/Qdata/nativephp.sqlite' }, false)).toBe(false)
    expect(offersImportHint({ ...info, chosenPath: '/Qchosen/nativephp.sqlite' }, false)).toBe(false)
    expect(offersImportHint({ ...info, running: true }, false)).toBe(false)
  })
})
