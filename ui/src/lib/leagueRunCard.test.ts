import { describe, expect, it } from 'vitest'
import {
  finishBars,
  gameRecordText,
  oddsNote,
  roundsLeftText,
  runCardTitle,
  signedNumber,
  winsForPrizeText,
} from './leagueRunCard'

describe('runCardTitle', () => {
  it('says whether the run is still being played', () => {
    expect(runCardTitle('active')).toBe('League in progress')
    expect(runCardTitle('complete')).toBe('Latest league')
    expect(runCardTitle('dropped')).toBe('Latest league')
  })
})

describe('gameRecordText', () => {
  it('is the games won and lost, with an en dash as 0.x', () => {
    expect(
      gameRecordText([
        { result: 'W', onPlay: true },
        { result: 'L', onPlay: false },
        { result: 'W', onPlay: null },
      ]),
    ).toBe('2–1')
    expect(gameRecordText([])).toBe('0–0')
  })
})

describe('roundsLeftText', () => {
  it('counts the rounds left', () => {
    expect(roundsLeftText(1)).toBe('1 round left')
    expect(roundsLeftText(3)).toBe('3 rounds left')
  })
})

describe('winsForPrizeText', () => {
  it('counts the wins still needed for a prize', () => {
    expect(winsForPrizeText(1)).toBe('1 win')
    expect(winsForPrizeText(2)).toBe('2 wins')
  })

  it('is hidden when no win is needed or no prize can be reached', () => {
    expect(winsForPrizeText(0)).toBeNull()
    expect(winsForPrizeText(null)).toBeNull()
  })
})

describe('oddsNote', () => {
  it('names where the projection came from, as 0.x', () => {
    expect(oddsNote('league', 62)).toBe('from a 62% league win rate')
    expect(oddsNote('deck', 55)).toBe("from this deck's 55% overall win rate")
    expect(oddsNote('none', 50)).toBe('from an even chance each round, for want of a record')
  })
})

describe('signedNumber', () => {
  it('puts a plus on gains only', () => {
    expect(signedNumber(12)).toBe('+12')
    expect(signedNumber(3.5)).toBe('+3.5')
    expect(signedNumber(0)).toBe('0')
    expect(signedNumber(-4)).toBe('-4')
  })
})

describe('finishBars', () => {
  const finishes = {
    buckets: [
      { label: '5-0', count: 1 },
      { label: '4-1', count: 2 },
      { label: '3-2', count: 0 },
      { label: '2-3', count: 0 },
      { label: '1-4', count: 0 },
      { label: '0-5', count: 1 },
    ],
    dropped: 4,
  }

  it('lists the finishes from 5-0 down, then the drops', () => {
    expect(finishBars(finishes).map((bar) => [bar.label, bar.count])).toEqual([
      ['5-0', 1],
      ['4-1', 2],
      ['3-2', 0],
      ['2-3', 0],
      ['1-4', 0],
      ['0-5', 1],
      ['Drop', 4],
    ])
  })

  it('sizes each bar against every run, drops included', () => {
    const bars = finishBars(finishes)
    expect(bars[0]!.percent).toBe(12.5)
    expect(bars[1]!.percent).toBe(25)
    expect(bars[6]!.percent).toBe(50)
  })

  it('colours winning finishes green and losing ones and drops red', () => {
    const bars = finishBars(finishes)
    expect(bars[0]!.barClass).toBe('bg-success')
    expect(bars[2]!.barClass).toBe('bg-success')
    expect(bars[3]!.barClass).toBe('bg-destructive')
    expect(bars[6]!.barClass).toBe('bg-destructive')
  })

  it('shows empty bars with no runs', () => {
    const empty = { buckets: finishes.buckets.map((b) => ({ ...b, count: 0 })), dropped: 0 }
    expect(finishBars(empty).every((bar) => bar.percent === 0)).toBe(true)
  })
})
