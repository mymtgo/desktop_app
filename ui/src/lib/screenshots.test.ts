import { describe, expect, it } from 'vitest'
import type { CardStatView, ScreenshotCardView } from '@/bindings'
import { DEFAULT_VISIBILITY, type Visibility } from '@/lib/cardStats/columns'
import {
  CARD_HEIGHT,
  boardChip,
  cardStatBlocks,
  cmcBarHeight,
  cmcMax,
  contextChips,
  deckFooterText,
  durationText,
  groupIntoStacks,
  groupSideboardStacks,
  leagueScoreText,
  playDrawChip,
  playSideText,
  resultText,
  screenshotFileName,
  threeGames,
  toneFor,
} from './screenshots'

function card(name: string, quantity: number): ScreenshotCardView {
  return { name, type: 'Instant', quantity, imageBase64: `data:image/jpeg;base64,${name}` }
}

/** A card stat with every count zero, then `fields`. */
function statOf(fields: Partial<CardStatView>): CardStatView {
  const zero: Record<string, unknown> = {}
  for (const key of [
    'catalogId',
    'totalGames',
    'totalPossible',
    'totalKept',
    'keptGames',
    'keptWon',
    'keptLost',
    'totalSeen',
    'seenGames',
    'seenWon',
    'seenLost',
    'totalCast',
    'castGames',
    'castWon',
    'castLost',
    'postboardGames',
    'sidedOutGames',
    'sidedInGames',
    'totalPlayed',
    'playedGames',
    'totalKicked',
    'totalActivated',
    'pregameRevealedGames',
    'pregamePlayedGames',
    'pregameGames',
    'pregameWon',
    'pregameLost',
    'inferredGames',
  ]) {
    zero[key] = 0
  }
  return {
    ...(zero as unknown as CardStatView),
    key: 'q-oracle',
    oracleId: 'q-oracle',
    name: 'Qbolt',
    colorIdentity: 'R',
    type: 'Instant',
    image: null,
    localImage: null,
    isSideboard: false,
    ...fields,
  }
}

function only(keys: (keyof Visibility)[]): Visibility {
  const columns = { ...DEFAULT_VISIBILITY }
  for (const key of Object.keys(columns) as (keyof Visibility)[]) columns[key] = keys.includes(key)
  return columns
}

describe('the deck screenshot (0.x DeckScreenshot)', () => {
  it('stacks each card by its quantity, 22px apart', () => {
    const stacks = groupIntoStacks([card('Qbolt', 4), card('Qgoblin', 1)])
    expect(stacks.map((stack) => stack.items.length)).toEqual([4, 1])
    expect(stacks[0].height).toBe(CARD_HEIGHT + 3 * 22)
    expect(stacks[1].height).toBe(CARD_HEIGHT)
    expect(CARD_HEIGHT).toBe(139)
    expect(stacks[0].items[0]).toEqual({ name: 'Qbolt', imageBase64: 'data:image/jpeg;base64,Qbolt' })
  })

  it('packs the sideboard into stacks of up to four copies', () => {
    const stacks = groupSideboardStacks([card('Qa', 3), card('Qb', 2), card('Qc', 1)])
    expect(stacks.map((stack) => stack.items.map((item) => item.name))).toEqual([
      ['Qa', 'Qa', 'Qa', 'Qb'],
      ['Qb', 'Qc'],
    ])
    expect(stacks[1].height).toBe(CARD_HEIGHT + 22)
    expect(groupSideboardStacks([])).toEqual([])
  })

  it('scales the curve to its tallest bucket', () => {
    const buckets = [{ count: 0 }, { count: 8 }, { count: 1 }]
    expect(cmcMax(buckets)).toBe(8)
    expect(cmcMax([{ count: 0 }])).toBe(1)
    expect(cmcBarHeight(8, 8)).toBe(40)
    expect(cmcBarHeight(1, 80)).toBe(3)
    expect(cmcBarHeight(0, 8)).toBe(0)
  })

  it('counts the main deck and sideboard in the footer', () => {
    expect(deckFooterText([card('Qa', 4), card('Qb', 32)], [card('Qland', 24)], [card('Qs', 15)])).toBe(
      '60 cards main · 15 sideboard',
    )
  })
})

describe('screenshot file names', () => {
  it('slugs the name with the kind', () => {
    expect(screenshotFileName('Qburn Deluxe!', 'deck')).toBe('qburn-deluxe-deck.png')
    expect(screenshotFileName('Qbolt', 'card')).toBe('qbolt-card.png')
    expect(screenshotFileName(null, 'league')).toBe('league.png')
    expect(screenshotFileName('***', 'league')).toBe('league.png')
  })
})

describe('the league screenshot (0.x LeagueScreenshot)', () => {
  it('scores the run and names results', () => {
    expect(leagueScoreText(['W', 'L', 'W', null, null])).toBe('2-1')
    expect(resultText('W')).toBe('Win')
    expect(resultText('L')).toBe('Loss')
  })

  it('shows games 1 to 3 with the play side, and the match time', () => {
    const games = threeGames([
      { result: 'W', onPlay: true },
      { result: 'L', onPlay: false },
    ])
    expect(games[2]).toBeNull()
    expect(playSideText(games[0]!)).toBe('(OTP)')
    expect(playSideText(games[1]!)).toBe('(OTD)')
    expect(playSideText({ result: 'W', onPlay: null })).toBeNull()
    expect(durationText(1530)).toBe('26m')
    expect(durationText(null)).toBe('-')
    expect(durationText(0)).toBe('-')
  })
})

describe('the card stat screenshot (0.x CardStatScreenshot)', () => {
  it('colours win rates above 55% and below 45%', () => {
    expect(toneFor(56)).toBe('good')
    expect(toneFor(55)).toBe('neutral')
    expect(toneFor(45)).toBe('neutral')
    expect(toneFor(44)).toBe('bad')
    expect(toneFor(null)).toBe('neutral')
  })

  it('builds a block per visible column in 0.x order', () => {
    const stat = statOf({
      totalGames: 10,
      keptGames: 6,
      keptWon: 4,
      keptLost: 2,
      castGames: 5,
      castWon: 2,
      castLost: 3,
      playedGames: 5,
      totalKicked: 2,
      seenGames: 8,
      seenWon: 4,
      seenLost: 4,
      postboardGames: 4,
      sidedOutGames: 1,
      sidedInGames: 0,
    })
    expect(cardStatBlocks(stat, DEFAULT_VISIBILITY)).toEqual([
      { label: 'Kept', value: '60%', sub: '(6)' },
      { label: 'Kept Win', value: '67%', sub: '(6)', tone: 'good' },
      { label: 'Cast', value: '50%', sub: '(5)' },
      { label: 'Cast Win', value: '40%', sub: '(5)', tone: 'bad' },
      { label: 'Played', value: '50%', sub: '(5)' },
      { label: 'Kicked', value: '2' },
      { label: 'Activated', value: '-' },
      { label: 'Pregame', value: '-', sub: undefined },
      { label: 'Pregame Win', value: '-', sub: '(0)', tone: 'neutral' },
      { label: 'Seen', value: '80%', sub: '(8)' },
      { label: 'Seen Win', value: '50%', sub: '(8)', tone: 'neutral' },
      { label: 'SB Out', value: '25%', sub: '(1)' },
      { label: 'SB In', value: '0%', sub: '(0)' },
      { label: 'Games', value: '10' },
    ])
    expect(cardStatBlocks(stat, only(['games', 'pregamePct']))).toEqual([
      { label: 'Pregame', value: '-', sub: undefined },
      { label: 'Games', value: '10' },
    ])
    expect(cardStatBlocks(statOf({}), only(['keptPct']))).toEqual([{ label: 'Kept', value: '-', sub: '(0)' }])
  })

  it('labels the filters as chips, the timeframe last', () => {
    expect(boardChip('game1')).toBe('Game 1')
    expect(boardChip('postboard')).toBe('Postboard')
    expect(boardChip('__all__')).toBeNull()
    expect(playDrawChip('play')).toBe('On the play')
    expect(playDrawChip('draw')).toBe('On the draw')
    expect(playDrawChip('__all__')).toBeNull()
    expect(contextChips('Postboard', 'On the draw', 'Last 30 days')).toEqual(['Postboard', 'On the draw', 'Last 30 days'])
    expect(contextChips(null, null, 'All time')).toEqual(['All time'])
  })
})
