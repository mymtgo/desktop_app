import { describe, expect, it } from 'vitest'
import {
  baselinePct,
  estimatedKind,
  estimatedTooltip,
  inferredTooltip,
  logOnlyNote,
  pct,
  rawWinPctLabel,
  regenerateMessage,
  REGENERATE_FAILED_MESSAGE,
  TRUST_DEFAULT,
  trustMax,
  winRateClass,
} from './format'

const rates = { kept: 0.5, cast: 0.82, seen: null, pregame: null }
const samples = { kept: 2, cast: 11, seen: 0, pregame: 0 }

describe('pct', () => {
  it('rounds to a whole percent', () => {
    expect(pct(1, 3)).toBe(33)
    expect(pct(2, 3)).toBe(67)
  })

  it('is null without a denominator', () => {
    expect(pct(0, 0)).toBeNull()
  })
})

describe('rawWinPctLabel', () => {
  it('says no data without games', () => {
    expect(rawWinPctLabel(rates, samples, 'seen', 50, 0.5, 'mine')).toBe('no data')
  })

  it('shows the raw rate and the baseline when there is trust', () => {
    expect(rawWinPctLabel(rates, samples, 'cast', 50, 0.48, 'mine')).toBe(
      'Raw 82% over 11 games · adjusted toward 48% deck baseline',
    )
  })

  it('leaves the baseline out at trust 0 and says game for one', () => {
    expect(rawWinPctLabel(rates, { ...samples, kept: 1 }, 'kept', 0, 0.48, 'mine')).toBe('Raw 50% over 1 game')
  })

  it('shows the baseline from their side for their cards', () => {
    expect(baselinePct(0.48, 'theirs')).toBe(52)
    expect(rawWinPctLabel(rates, samples, 'cast', 10, 0.48, 'theirs')).toContain('adjusted toward 52% deck baseline')
  })
})

describe('winRateClass', () => {
  it('colours my cards above 55 green and below 45 red', () => {
    expect(winRateClass(56, 'mine')).toBe('text-success')
    expect(winRateClass(44, 'mine')).toBe('text-destructive')
    expect(winRateClass(55, 'mine')).toBe('')
    expect(winRateClass(45, 'mine')).toBe('')
  })

  it('inverts the colours for their cards', () => {
    expect(winRateClass(56, 'theirs')).toBe('text-destructive')
    expect(winRateClass(44, 'theirs')).toBe('text-success')
    expect(winRateClass(50, 'theirs')).toBe('')
  })
})

describe('trustMax', () => {
  it('is 200 for a small deck and with no games', () => {
    expect(trustMax(0)).toBe(200)
    expect(trustMax(100)).toBe(200)
  })

  it('grows in steps of 50 at twice the games', () => {
    expect(trustMax(101)).toBe(250)
    expect(trustMax(300)).toBe(600)
  })

  it('keeps the default at 50', () => {
    expect(TRUST_DEFAULT).toBe(50)
  })
})

describe('messages', () => {
  it('words the regenerate result as 0.x', () => {
    expect(regenerateMessage(0)).toBe('No matches needed regeneration.')
    expect(regenerateMessage(1)).toBe('Queued regeneration for 1 match. Stats will refresh shortly.')
    expect(regenerateMessage(3)).toBe('Queued regeneration for 3 matches. Stats will refresh shortly.')
  })

  it('words the regenerate failure as 0.x, with nothing appended', () => {
    expect(REGENERATE_FAILED_MESSAGE).toBe('Could not regenerate card stats.')
  })

  it('words the inferred marker tooltip', () => {
    expect(inferredTooltip(1)).toBe(
      'Includes 1 imported game where only cards seen in the game log count as sided in.',
    )
    expect(inferredTooltip(4)).toBe(
      'Includes 4 imported games where only cards seen in the game log count as sided in.',
    )
  })

  it('words the note above the table, singular and plural', () => {
    expect(logOnlyNote(3)).toBe(
      "3 games here are from imported matches with only MTGO's game log. Seen counts only cards that were cast or played, and sideboard numbers are partial.",
    )
    expect(logOnlyNote(1)).toBe(
      "1 game here is from an imported match with only MTGO's game log. Seen counts only cards that were cast or played, and sideboard numbers are partial.",
    )
  })

  it('picks the columns that are estimated', () => {
    expect(estimatedKind('seenPct')).toBe('seen')
    expect(estimatedKind('seenWinPct')).toBe('seen')
    expect(estimatedKind('sbOutPct')).toBe('sideboard')
    expect(estimatedKind('sbInPct')).toBe('sideboard')
    expect(estimatedKind('castPct')).toBeNull()
    expect(estimatedKind('games')).toBeNull()
  })

  it('words the estimated marker tooltips', () => {
    expect(estimatedTooltip('seen', 1)).toBe(
      "Estimated. 1 game here is from an imported match with only MTGO's game log, which never shows your hand, so a card counts as seen only when it was cast or played.",
    )
    expect(estimatedTooltip('sideboard', 4)).toBe(
      "Estimated. 4 games here are from imported matches with only MTGO's game log, so sideboarding is only visible for cards that show up in the log.",
    )
  })
})
