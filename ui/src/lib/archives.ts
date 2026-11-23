// Settings, Archives (league self-management spec 5.5): the deleted matches
// and leagues, each with Restore.

import type { SummaryOutcome } from '@/bindings'

/** A deleted match's result in the Matches table. */
export function deletedResultText(result: SummaryOutcome): string {
  if (result === 'win') return 'Win'
  if (result === 'loss') return 'Loss'
  if (result === 'draw') return 'Draw'
  return '-'
}

/** The colour of a deleted match's result badge. */
export function resultTone(result: SummaryOutcome): 'win' | 'loss' | 'draw' | 'muted' {
  return result === 'unknown' ? 'muted' : result
}

/** A deleted league's record. */
export function recordText(wins: number, losses: number): string {
  return `${wins}-${losses}`
}

/** A tab's label with the number of rows it holds. */
export function archiveTabLabel(label: string, count: number): string {
  return `${label} (${count})`
}

/** The Deck cell of an Archives row: the deck name, else the format (muted), else "-" (muted). */
export function deckCell(deckName: string | null, format: string | null): { text: string; muted: boolean } {
  if (deckName) return { text: deckName, muted: false }
  if (format) return { text: format, muted: true }
  return { text: '-', muted: true }
}
