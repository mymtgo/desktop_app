import type { BackgroundImport, ImportProgress, ImportSummary, LegacyImportInfo } from '@/bindings'

/** Which 0.x database the card will import from. */
export function pathLine(info: LegacyImportInfo): string {
  if (info.chosenPath !== null) return `Using ${info.chosenPath}`
  if (info.detectedPath !== null) return `Found at ${info.detectedPath}`
  return 'No 0.x database found in the usual place. Choose its file (database.sqlite or nativephp.sqlite).'
}

/** The first-run screen offers the 0.x import: a 0.x database is on this computer and nothing was imported yet. */
export function offersFirstImport(info: LegacyImportInfo | undefined): boolean {
  return info !== undefined && info.detectedPath !== null && info.lastImport === null
}

/**
 * The dashboard hint for a new user with no 0.x database detected: nothing
 * detected or chosen, nothing imported or importing, and no v1 match yet.
 */
export function offersImportHint(info: LegacyImportInfo | undefined, hasMatches: boolean): boolean {
  if (info === undefined || hasMatches) return false
  return info.detectedPath === null && info.chosenPath === null && info.lastImport === null && !info.running
}

/** One line for the current phase of an import. */
export function progressLine(progress: ImportProgress): string {
  switch (progress.phase) {
    case 'reading':
      return 'Reading 0.x data...'
    case 'writing':
      if (progress.total === 0) return 'Importing matches...'
      return `Importing ${progress.done} of ${progress.total} ${progress.total === 1 ? 'match' : 'matches'}`
    case 'details':
      if (progress.total === 0) return 'Importing match detail...'
      return `Importing match detail: ${progress.done} of ${progress.total}`
    case 'decks':
      return 'Importing decks...'
    case 'leagues':
      return 'Importing leagues...'
    case 'limited':
      return 'Importing Limited drafts...'
    case 'archetypes':
      return 'Importing archetypes and sideboard guides...'
    case 'finished':
      return 'Import finished'
    case 'failed':
      return progress.message
  }
}

/**
 * Whether an import is running: every phase before `finished` or `failed`
 * counts, so a new phase never hides the progress. Without a progress event
 * yet, the stored info decides.
 */
export function importRunning(progress: ImportProgress | null | undefined, infoRunning: boolean): boolean {
  if (progress === null || progress === undefined) return infoRunning
  return progress.phase !== 'finished' && progress.phase !== 'failed'
}

/** How full the progress bar is (0 to 100), or null when the phase has no bar. */
export function progressPercent(progress: ImportProgress): number | null {
  if (progress.phase !== 'writing' && progress.phase !== 'details') return null
  if (progress.total === 0) return null
  return Math.round((progress.done / progress.total) * 100)
}

function ago(thenMs: number, nowMs: number): string {
  const minutes = Math.round((nowMs - thenMs) / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  const days = Math.round(hours / 24)
  return days === 1 ? '1 day ago' : `${days} days ago`
}

/** "Last import 5 min ago: 3 added, 1 updated, 0 unchanged". */
export function summaryLines(summary: ImportSummary, nowMs: number): string {
  return `Last import ${ago(summary.finishedAtMs, nowMs)}: ${summary.added} added, ${summary.updated} updated, ${summary.unchanged} unchanged`
}

/** The non-zero skips, or null when nothing was skipped. */
export function skipsLine(summary: ImportSummary): string | null {
  const parts: string[] = []
  if (summary.skippedLive > 0) parts.push(`${summary.skippedLive} already tracked live`)
  if (summary.skippedInProgress > 0) parts.push(`${summary.skippedInProgress} still in progress in 0.x`)
  if (summary.skippedNoLocalPlayer > 0) parts.push(`${summary.skippedNoLocalPlayer} without a local player`)
  // Missing from a summary stored before it was counted (#337).
  const notAccount = summary.skippedNotAccount ?? 0
  if (notAccount > 0) {
    parts.push(
      notAccount === 1
        ? '1 from a player that was not one of your 0.x accounts'
        : `${notAccount} from players that were not your 0.x accounts`,
    )
  }
  if (summary.skippedDuplicateToken > 0) parts.push(`${summary.skippedDuplicateToken} duplicates`)
  if (summary.skippedBadTime > 0) parts.push(`${summary.skippedBadTime} with a bad start time`)
  // Missing from a summary stored before they were counted.
  const blank = summary.skippedBlankToken ?? 0
  if (blank > 0) parts.push(`${blank} without a match token`)
  const repeated = summary.skippedDuplicateGames ?? 0
  if (repeated > 0) parts.push(`${repeated} repeated ${repeated === 1 ? 'game' : 'games'}`)
  const details = summary.skippedDetails ?? 0
  if (details > 0) {
    parts.push(`${plural(details, 'match detail', 'match details')} skipped (import again to retry)`)
  }
  return parts.length === 0 ? null : parts.join(', ')
}

/** What the last import did with the 0.x decks, or null for a summary stored before decks were imported. */
export function decksLine(summary: ImportSummary): string | null {
  const decks = summary.decks
  if (decks === undefined || decks === null) return null
  if (!decks.tablesFound) return 'This 0.x database has no decks, so only matches were imported.'
  return (
    `Decks: ${decks.decksAdded} added, ${decks.decksUpdated} updated, ${decks.decksUnchanged} unchanged. ` +
    `Versions: ${decks.versionsAdded} added. ` +
    `Cards: ${decks.cardsAdded} added, ${decks.cardsUpdated} updated. ` +
    `Match links: ${decks.linksAdded} added.`
  )
}

/** The 0.x deck data left out, or null when nothing was. */
export function deckSkipsLine(summary: ImportSummary): string | null {
  const decks = summary.decks
  if (decks === undefined || decks === null) return null
  // Limited decks are not skipped any more: their versions come over as the
  // registered decks of their matches (see `limitedLine`). A summary stored
  // before Limited was imported still says they were left out (#194).
  const parts: string[] = []
  const limitedDecks = decks.decksSkippedLimited
  if ((summary.limited === undefined || summary.limited === null) && limitedDecks > 0) {
    parts.push(plural(limitedDecks, 'Limited deck', 'Limited decks'))
  }
  const duplicates = decks.decksSkippedDuplicate ?? 0
  if (duplicates > 0) parts.push(`${duplicates} ${duplicates === 1 ? 'deck' : 'decks'} sharing another deck's MTGO id`)
  const versions = decks.versionsSkipped
  if (versions > 0) parts.push(`${versions} old deck ${versions === 1 ? 'version' : 'versions'} that could not be read`)
  return parts.length === 0 ? null : `Not imported: ${parts.join(', ')}`
}

function plural(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`
}

/**
 * What the last import did with 0.x's shared replay links, or null when there were none (or for a summary
 * stored before they were counted). A link that could not go on a match here (its match was not imported, is
 * tracked here under another token, or already holds a link) and pre-release per-game links stay public, so
 * the line says where to switch them off (#307 F1).
 */
export function replaysLine(summary: ImportSummary): string | null {
  const added = summary.replaySharesAdded ?? 0
  const incomplete = summary.replaySharesIncomplete ?? 0
  const unplaced = summary.replaySharesUnplaced ?? 0
  const perGame = summary.legacyGameLinks ?? 0
  if (added + incomplete + unplaced + perGame === 0) return null
  const sentences: string[] = []
  if (added > 0) sentences.push(`${plural(added, 'shared replay link', 'shared replay links')} brought over.`)
  if (incomplete > 0) {
    sentences.push(`${plural(incomplete, 'incomplete link was', 'incomplete links were')} left out.`)
  }
  const behind = unplaced + perGame
  if (behind > 0) {
    const one = behind === 1
    const kinds: string[] = []
    if (unplaced > 0) {
      kinds.push(
        unplaced === 1
          ? '1 whose match was not imported, is tracked here under another ID or already has a link'
          : `${unplaced} whose matches were not imported, are tracked here under another ID or already have a link`,
      )
    }
    if (perGame > 0) {
      kinds.push(`${plural(perGame, 'per-game link', 'per-game links')} from a pre-release build`)
    }
    sentences.push(
      `${plural(behind, 'replay link stays', 'replay links stay')} shared on mymtgo.com but ` +
        `${one ? 'is' : 'are'} not linked to a match here ` +
        `(${kinds.join('; ')}): switch ${one ? 'it' : 'them'} off on your Account page on mymtgo.com.`,
    )
  }
  return `Replays: ${sentences.join(' ')}`
}

/** What the last import did with the 0.x leagues, or null without a leagues table or for a summary stored before leagues were imported. */
export function leaguesLine(summary: ImportSummary): string | null {
  const leagues = summary.leagues
  if (leagues === undefined || leagues === null || !leagues.tablesFound) return null
  const line = `Leagues: ${leagues.added} added, ${leagues.updated} updated, ${leagues.unchanged} unchanged.`
  if (leagues.withoutAccount === 0) return line
  return `${line} ${plural(leagues.withoutAccount, 'league', 'leagues')} could not be matched to an account.`
}

/** The 0.x leagues left out, or null when none were. */
export function leagueSkipsLine(summary: ImportSummary): string | null {
  const leagues = summary.leagues
  if (leagues === undefined || leagues === null) return null
  const parts: string[] = []
  // A summary stored before Limited was imported left draft and sealed leagues out (#194).
  const limitedLeagues = leagues.skippedLimited ?? 0
  if ((summary.limited === undefined || summary.limited === null) && limitedLeagues > 0) {
    parts.push(plural(limitedLeagues, 'Limited league', 'Limited leagues'))
  }
  if (leagues.skippedDeleted > 0) parts.push(plural(leagues.skippedDeleted, 'deleted league', 'deleted leagues'))
  if (leagues.skippedUnreadable > 0) {
    parts.push(plural(leagues.skippedUnreadable, 'unreadable league', 'unreadable leagues'))
  }
  const notAccount = leagues.skippedNotAccount ?? 0
  if (notAccount > 0) {
    parts.push(
      plural(
        notAccount,
        'league of a player that was not one of your 0.x accounts',
        'leagues of players that were not your 0.x accounts',
      ),
    )
  }
  return parts.length === 0 ? null : `Leagues not imported: ${parts.join(', ')}`
}

/** What the last import did with the 0.x accounts, or null for a summary stored before accounts were imported. */
export function accountsLine(summary: ImportSummary): string | null {
  const accounts = summary.accounts
  if (accounts === undefined || accounts === null) return null
  // An old 0.x without an accounts table imports none: nothing to report.
  if (accounts.added + accounts.unchanged + accounts.loginIdsFilled + accounts.loginIdClashes === 0) return null
  let line = `Accounts: ${accounts.added} added, ${accounts.unchanged} unchanged.`
  if (accounts.loginIdsFilled > 0) {
    line += ` ${plural(accounts.loginIdsFilled, 'login id', 'login ids')} filled.`
  }
  if (accounts.loginIdClashes > 0) {
    const verb = accounts.loginIdClashes === 1 ? 'belongs' : 'belong'
    line += ` ${plural(accounts.loginIdClashes, 'login id', 'login ids')} already ${verb} to another account.`
  }
  // Both are missing from a summary stored before they were counted.
  const untracked = accounts.untracked ?? 0
  if (untracked > 0) {
    line += ` ${plural(untracked, 'account', 'accounts')} not recording matches, as in 0.x.`
  }
  if (accounts.activeSeeded === true) {
    line += ' Current account set from 0.x.'
  }
  return line
}

/** What the last import did with the 0.x drafts, pick notes and registered decks, or null without Limited tables or for a summary stored before Limited was imported. */
export function limitedLine(summary: ImportSummary): string | null {
  const limited = summary.limited
  if (limited === undefined || limited === null || !limited.tablesFound) return null
  let line =
    `Limited: ${plural(limited.draftsAdded, 'draft', 'drafts')} added, ${limited.draftsUpdated} updated, ` +
    `${limited.draftsUnchanged} unchanged. ${plural(limited.notesAdded, 'pick note', 'pick notes')} added. ` +
    `Registered decks: ${limited.decksAdded} added, ${limited.decksUpdated} updated, ${limited.decksUnchanged} unchanged.`
  if (limited.draftsWithoutAccount > 0) {
    line += ` ${plural(limited.draftsWithoutAccount, 'draft', 'drafts')} could not be matched to an account.`
  }
  return line
}

/** The 0.x Limited data left out, or null when none was. */
export function limitedSkipsLine(summary: ImportSummary): string | null {
  const limited = summary.limited
  if (limited === undefined || limited === null) return null
  const parts: string[] = []
  if (limited.draftsSkipped > 0) parts.push(plural(limited.draftsSkipped, 'unreadable draft', 'unreadable drafts'))
  const notAccount = limited.draftsNotAccount ?? 0
  if (notAccount > 0) {
    parts.push(
      plural(
        notAccount,
        'draft of a player that was not one of your 0.x accounts',
        'drafts of players that were not your 0.x accounts',
      ),
    )
  }
  if (limited.decksWithoutRun > 0) {
    parts.push(
      plural(
        limited.decksWithoutRun,
        'registered deck of a league not imported',
        'registered decks of leagues not imported',
      ),
    )
  }
  if (limited.decksSkipped > 0) {
    parts.push(plural(limited.decksSkipped, 'unreadable registered deck', 'unreadable registered decks'))
  }
  // Older stored summaries have no count.
  const notesSkipped = limited.notesSkipped ?? 0
  if (notesSkipped > 0) {
    parts.push(
      `${plural(notesSkipped, 'pick note of a pick', 'pick notes of picks')} not read from the log yet (import again later)`,
    )
  }
  return parts.length === 0 ? null : `Limited not imported: ${parts.join(', ')}`
}

/** What the last import brought over of 0.x's MyMTGO API state, or null for a summary stored before it was imported. */
export function apiLine(summary: ImportSummary): string | null {
  const api = summary.api
  if (api === undefined || api === null) return null
  let line =
    `MyMTGO: ${plural(api.reportsSent, 'match report', 'match reports')} already sent, ` +
    `${plural(api.cardStatsAdded, 'card stats game', 'card stats games')} queued, ` +
    `${plural(api.syncedRows, 'synced row', 'synced rows')} and ` +
    `${plural(api.tournamentsNamed, 'tournament name', 'tournament names')} brought over.`
  if (api.cardStatsSkipped > 0) {
    line += ` ${plural(api.cardStatsSkipped, 'card stats game', 'card stats games')} of matches not imported were left out.`
  }
  return line
}

/** What the last import brought over of 0.x's archetypes, match and deck archetypes, sideboard guides and matchup notes, or null for a summary stored before they were imported. */
export function archetypesLine(summary: ImportSummary): string | null {
  const archetypes = summary.archetypes
  if (archetypes === undefined || archetypes === null) return null
  let line =
    `Archetypes: ${archetypes.archetypesAdded} added, ${archetypes.archetypesUpdated} updated, ` +
    `${archetypes.archetypesUnchanged} unchanged. ` +
    `${plural(archetypes.matchRowsAdded, 'match archetype', 'match archetypes')}, ` +
    `${plural(archetypes.decksLinked, 'deck archetype', 'deck archetypes')}, ` +
    `${plural(archetypes.guidesAdded, 'sideboard guide', 'sideboard guides')} and ` +
    `${plural(archetypes.notesAdded, 'matchup note', 'matchup notes')} brought over.`
  if (archetypes.matchRowsKept > 0) {
    const verb = archetypes.matchRowsKept === 1 ? 'was' : 'were'
    line += ` ${plural(archetypes.matchRowsKept, 'match archetype', 'match archetypes')} already set in v1 ${verb} kept.`
  }
  if (archetypes.matchRowsSkipped > 0) {
    const one = archetypes.matchRowsSkipped === 1
    line +=
      ` ${plural(archetypes.matchRowsSkipped, 'match archetype', 'match archetypes')} ${one ? 'was' : 'were'} left out: ` +
      `${one ? 'its' : 'their'} match, player or archetype is not in v1.`
  }
  if (archetypes.guidesKept > 0) {
    const verb = archetypes.guidesKept === 1 ? 'was' : 'were'
    line += ` ${plural(archetypes.guidesKept, 'sideboard guide', 'sideboard guides')} v1 already had ${verb} kept.`
  }
  if (archetypes.notesSkipped > 0) {
    const one = archetypes.notesSkipped === 1
    line +=
      ` ${plural(archetypes.notesSkipped, 'matchup note', 'matchup notes')} ${one ? 'was' : 'were'} left out: ` +
      `${one ? 'its' : 'their'} deck or archetype is not in v1, or ${one ? 'it is' : 'they are'} empty or too long.`
  }
  return line
}

/** The Settings import card's note on archetype detection (archetypes spec section 15). */
export const ARCHETYPE_WAIT_NOTE =
  'Imported matches get their archetypes after the import finishes; if reading 0.x fails, they wait for a successful import.'

const backgroundLines: Record<BackgroundImport, string | null> = {
  not_set: null,
  copied: 'The custom league card background was copied.',
  kept: "v1's own league card background was kept.",
  missing: 'The custom league card background file was not found in 0.x, so it was skipped.',
  failed: 'The custom league card background could not be copied from 0.x, so it was skipped.',
  refused: 'The custom league card background is not a jpeg, png, webp or gif of at most 5 MB, so it was skipped.',
}

/** What the last import brought over of 0.x's overlay deck labels and custom background, or null for a summary stored before they were imported. */
export function overlayLine(summary: ImportSummary): string | null {
  const overlay = summary.overlay
  if (overlay === undefined || overlay === null) return null
  let line = `Overlay: ${plural(overlay.labelsAdded, 'deck label', 'deck labels')} brought over.`
  if (overlay.labelsKept > 0) {
    const verb = overlay.labelsKept === 1 ? 'was' : 'were'
    line += ` ${plural(overlay.labelsKept, 'deck label', 'deck labels')} already set in v1 ${verb} kept.`
  }
  if (overlay.labelsUnknown > 0) {
    const decks = overlay.labelsUnknown === 1 ? 'a deck' : 'decks'
    const verb = overlay.labelsUnknown === 1 ? 'was' : 'were'
    line += ` ${plural(overlay.labelsUnknown, 'deck label', 'deck labels')} of ${decks} not imported ${verb} left out.`
  }
  const background = backgroundLines[overlay.background]
  if (background !== null) line += ` ${background}`
  return line
}
