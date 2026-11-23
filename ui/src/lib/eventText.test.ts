import { describe, expect, it } from 'vitest'
import {
  approxTime,
  eventLabels,
  finishLabel,
  formatRecord,
  inTime,
  kindLabel,
  nextStage,
  ordinal,
  placeText,
  stageLabel,
  stageShort,
} from './eventText'

const NOW = 1_800_000_000_000
const MINUTE = 60_000

describe('ordinals', () => {
  it('reads 1st to 111th', () => {
    const numbers = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111]
    expect(numbers.map(ordinal)).toEqual(['1st', '2nd', '3rd', '4th', '11th', '12th', '13th', '21st', '22nd', '101st', '111th'])
  })

  it('names a place with the player count when known', () => {
    expect(placeText(12, 64)).toBe('12th of 64')
    expect(placeText(12, null)).toBe('12th')
  })
})

describe('records', () => {
  it('shows draws only when there are any', () => {
    expect(formatRecord({ wins: 3, losses: 1 })).toBe('3-1')
    expect(formatRecord({ wins: 3, losses: 1, draws: 0 })).toBe('3-1')
    expect(formatRecord({ wins: 3, losses: 1, draws: 1 })).toBe('3-1-1')
  })
})

describe('relative times', () => {
  it('counts whole minutes ahead, at least 1', () => {
    expect(inTime(NOW + 14 * MINUTE, NOW)).toBe('in ~14 min')
    expect(approxTime(NOW + 14 * MINUTE, NOW)).toBe('~14 min')
    expect(approxTime(NOW + 20_000, NOW)).toBe('~1 min')
    expect(approxTime(NOW + 14 * MINUTE + 25_000, NOW)).toBe('~14 min')
  })

  it('says soon once the estimate has passed', () => {
    expect(inTime(NOW, NOW)).toBe('soon')
    expect(inTime(NOW - MINUTE, NOW)).toBe('soon')
    expect(approxTime(NOW - MINUTE, NOW)).toBe('soon')
  })
})

describe('the event label (spec 5.2)', () => {
  const ON = { event: true, format: true }

  it('reads a league as its format and a tournament as format plus kind, then the kind alone', () => {
    // Competitive League (Modern), Modern League, Modern Challenge 64, Legacy Showcase Challenge,
    // Pioneer Preliminary, Modern Showcase Qualifier.
    expect(eventLabels('Modern', 'league', null, ON)).toEqual(['Modern'])
    expect(eventLabels('Modern', 'tournament', 'challenge', ON)).toEqual(['Modern Challenge', 'Challenge'])
    expect(eventLabels('Legacy', 'tournament', 'showcase_challenge', ON)).toEqual(['Legacy Showcase', 'Showcase'])
    expect(eventLabels('Pioneer', 'tournament', 'preliminary', ON)).toEqual(['Pioneer Prelim', 'Prelim'])
    expect(eventLabels('Modern', 'tournament', 'qualifier', ON)).toEqual(['Modern Qualifier', 'Qualifier'])
    expect(eventLabels('Modern', 'tournament', 'other', ON)).toEqual(['Modern Event', 'Event'])
  })

  it('falls back to the kind for every format word in the spec (the card shows the first that fits)', () => {
    for (const format of ['Modern', 'Pioneer', 'Legacy', 'Vintage', 'Standard', 'Pauper']) {
      expect(eventLabels(format, 'tournament', 'showcase_challenge', ON)).toEqual([`${format} Showcase`, 'Showcase'])
    }
  })

  it('uses a limited league kind as its format', () => {
    expect(eventLabels('Sealed', 'league', null, ON)).toEqual(['Sealed'])
    expect(eventLabels('Cube', 'league', null, ON)).toEqual(['Cube'])
  })

  it('leaves the kind with Format off', () => {
    expect(eventLabels('Modern', 'tournament', 'challenge', { event: true, format: false })).toEqual(['Challenge'])
    expect(eventLabels('Modern', 'league', null, { event: true, format: false })).toEqual(['League'])
  })

  it('leaves a tournament its format with Event name off; a league is unchanged', () => {
    expect(eventLabels('Modern', 'tournament', 'challenge', { event: false, format: true })).toEqual(['Modern'])
    expect(eventLabels('Modern', 'league', null, { event: false, format: true })).toEqual(['Modern'])
  })

  it('is empty with both off, and names the kind alone for an unknown format', () => {
    expect(eventLabels('Modern', 'league', null, { event: false, format: false })).toEqual([])
    expect(eventLabels('Modern', 'tournament', 'challenge', { event: false, format: false })).toEqual([])
    expect(eventLabels('', 'league', null, ON)).toEqual(['League'])
    expect(eventLabels(null, 'tournament', 'challenge', ON)).toEqual(['Challenge'])
    expect(eventLabels('  ', 'league', null, { event: false, format: true })).toEqual([])
  })

  it('never repeats the kind word', () => {
    expect(eventLabels('Vintage Challenge', 'tournament', 'challenge', ON)).toEqual(['Vintage Challenge', 'Challenge'])
  })
})

describe('kinds and stages', () => {
  it('labels the kind from the series', () => {
    expect(kindLabel('league', null)).toBe('League')
    expect(kindLabel('tournament', 'challenge')).toBe('Challenge')
    expect(kindLabel('tournament', 'showcase_challenge')).toBe('Showcase')
    expect(kindLabel('tournament', 'preliminary')).toBe('Prelim')
    expect(kindLabel('tournament', 'qualifier')).toBe('Qualifier')
    expect(kindLabel('tournament', 'other')).toBe('Event')
    expect(kindLabel('tournament', null)).toBe('Event')
  })

  it('names the playoff stages, the quarterfinal as Top 8', () => {
    expect(stageLabel('top8')).toBe('Top 8')
    expect(stageLabel('semifinal')).toBe('Semifinal')
    expect(stageLabel('final')).toBe('Final')
    expect(stageShort('top8')).toBe('Top 8')
    expect(stageShort('semifinal')).toBe('SF')
    expect(stageShort('final')).toBe('Final')
    expect(nextStage('top8')).toBe('semifinal')
    expect(nextStage('semifinal')).toBe('final')
  })

  it('names a top cut finish by the stage lost', () => {
    expect(finishLabel('top8')).toBe('Top 8')
    expect(finishLabel('semifinal')).toBe('Top 4')
    expect(finishLabel('final')).toBe('Finalist')
  })
})
