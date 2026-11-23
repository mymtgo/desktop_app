import type { MatchSummary, RowDeckView, SummaryOutcome } from '@/bindings'

/** A matches table row's deck, for tables with a deck column (the dashboard's Recent matches). */
export type MatchDeck = { id: number | null; name: string | null }

/** Each listed match's deck by token (`match_decks`); a match without one is left out. */
export function decksByToken(rows: RowDeckView[]): Record<string, MatchDeck> {
  const byToken: Record<string, MatchDeck> = {}
  for (const row of rows) byToken[row.token] = { id: row.deckId, name: row.deckName }
  return byToken
}

type Tone = 'win' | 'loss' | 'draw' | 'muted'

function byOutcome(outcome: SummaryOutcome, provisional: boolean): { text: string; tone: Tone; provisional: boolean } {
  switch (outcome) {
    case 'win':
      return { text: 'Win', tone: 'win', provisional }
    case 'loss':
      return { text: 'Loss', tone: 'loss', provisional }
    case 'draw':
      return { text: 'Draw', tone: 'draw', provisional }
    default:
      return { text: 'Unknown', tone: 'muted', provisional }
  }
}

/** The outcome cell. A live hint is provisional; only completion is final (spec 5.5). */
export function outcomeLabel(row: MatchSummary): { text: string; tone: Tone; provisional: boolean } {
  if (row.state === 'complete') {
    return byOutcome(row.outcome, false)
  }
  if (row.state === 'abandoned') {
    return { text: 'Abandoned', tone: 'muted', provisional: false }
  }
  if (row.tentativeOutcome !== null) {
    return byOutcome(row.tentativeOutcome, true)
  }
  return { text: 'In progress', tone: 'muted', provisional: true }
}

export function stateBadge(row: MatchSummary): string | null {
  if (row.state === 'in_progress') return 'Live'
  if (row.state === 'ended') return 'Ended'
  return null
}

/** A change with its sign: "+12", "-35", "0". */
export function signedNumber(value: number): string {
  return value > 0 ? `+${value}` : `${value}`
}
