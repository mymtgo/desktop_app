import { describe, expect, it } from 'vitest'
import type { DetailEventSource } from './matchDetail'
import type { GameDetailView, HandCardView, MatchDetail, RevealView } from '@/bindings'
import {
  aggregateReveals,
  changeLabel,
  durationText,
  gameTile,
  gamesCountText,
  groupReveals,
  handGridClass,
  heroResult,
  importedWithoutDetail,
  listenForDetailChanges,
  matchTimeText,
  matchesFallbackPath,
  mulliganStrip,
  NOTES_MAX,
  notesTooLong,
  openingHandTiles,
  openingHandTitle,
  playDrawText,
  previewTop,
  revealBorderStyle,
  revealFilters,
  revealType,
  ratingText,
  scoreText,
  showReplayButton,
  sideboardCount,
  startedAtText,
  turnsText,
} from './matchDetail'

function card(id: number, overrides: Partial<HandCardView> = {}): HandCardView {
  return { catalogId: id, name: `Card ${id}`, image: null, localImage: null, bottomed: false, firstDraw: false, ...overrides }
}

function hand(count: number): HandCardView[] {
  return Array.from({ length: count }, (_, i) => card(i + 1))
}

function reveal(name: string, overrides: Partial<RevealView> = {}): RevealView {
  return { catalogId: 1, name, image: null, localImage: null, typeLine: null, colors: [], quantity: 1, ...overrides }
}

function game(overrides: Partial<GameDetailView> = {}): GameDetailView {
  return {
    id: 1,
    number: 1,
    result: 'win',
    onThePlay: true,
    startedAtMs: null,
    endedAtMs: null,
    turns: null,
    localMulligans: 0,
    opponentMulligans: 0,
    openingHand: [],
    mulliganedHands: [],
    sideboardChanges: [],
    opponentCardsSeen: [],
    hasGameLog: false,
    hasDeckList: false,
    hasFrames: false,
    ...overrides,
  }
}

function detail(overrides: Partial<MatchDetail> = {}): MatchDetail {
  return {
    id: 1,
    token: 't1',
    platform: 'mtgo',
    source: 'log',
    state: 'complete',
    outcome: 'win',
    wins: 2,
    losses: 1,
    opponent: 'Qorvath',
    format: 'Modern',
    matchType: null,
    startedAtMs: 0,
    endedAtMs: null,
    leagueName: null,
    deck: null,
    opponentArchetype: null,
    archetypePending: false,
    notes: null,
    imported: false,
    manual: false,
    corrected: false,
    games: [],
    manualEditing: null,
    opponentCommanders: [],
    ...overrides,
  }
}

describe('imported match banner', () => {
  const imported = (games: GameDetailView[]) => detail({ imported: true, games })

  it('shows for an imported match whose games have no detail', () => {
    expect(importedWithoutDetail(imported([game(), game({ number: 2, hasGameLog: true, turns: 9 })]))).toBe(true)
    expect(importedWithoutDetail(imported([]))).toBe(true)
  })

  it('hides once any game has a hand, a deck list or reveals', () => {
    expect(importedWithoutDetail(imported([game(), game({ number: 2, openingHand: hand(7) })]))).toBe(false)
    expect(importedWithoutDetail(imported([game({ mulliganedHands: [hand(7)] })]))).toBe(false)
    expect(importedWithoutDetail(imported([game({ hasDeckList: true })]))).toBe(false)
    expect(importedWithoutDetail(imported([game({ opponentCardsSeen: [reveal('Qdragon')] })]))).toBe(false)
  })

  it('never shows for a tracked or manual match', () => {
    expect(importedWithoutDetail(detail({ games: [game()] }))).toBe(false)
    expect(importedWithoutDetail(detail({ manual: true, games: [game()] }))).toBe(false)
  })
})

describe('replay button', () => {
  it('shows for a log game with frames', () => {
    expect(showReplayButton(detail({ source: 'log' }), game({ hasFrames: true }))).toBe(true)
    expect(showReplayButton(detail({ source: 'imported', imported: true }), game({ hasFrames: true }))).toBe(true)
  })

  it('hides for manual and history matches and for frameless games', () => {
    expect(showReplayButton(detail({ source: 'manual', manual: true }), game({ hasFrames: true }))).toBe(false)
    expect(showReplayButton(detail({ source: 'history', imported: true }), game({ hasFrames: true }))).toBe(false)
    expect(showReplayButton(detail({ source: 'log' }), game({ hasFrames: false }))).toBe(false)
  })
})

describe('openingHandTiles (the hand and its pills)', () => {
  it('keeps the sorted order the app sends and marks the bottomed cards', () => {
    const tiles = openingHandTiles(game({ openingHand: [card(3), card(1, { bottomed: true }), card(2)] }))
    expect(tiles.map((tile) => tile.card.catalogId)).toEqual([3, 1, 2])
    expect(tiles.map((tile) => tile.pill)).toEqual([null, 'bottomed', null])
  })

  it('shows the turn 1 draw last with its pill', () => {
    const tiles = openingHandTiles(
      game({ onThePlay: false, openingHand: [card(4), card(9, { firstDraw: true }), card(5)] }),
    )
    expect(tiles.map((tile) => tile.card.catalogId)).toEqual([4, 5, 9])
    expect(tiles.map((tile) => tile.pill)).toEqual([null, null, 'draw'])
  })

  it('marks nothing without a draw or bottomed card', () => {
    expect(openingHandTiles(game({ openingHand: hand(7) })).every((tile) => tile.pill === null)).toBe(true)
  })

  it('lays out eight cards in eight columns', () => {
    expect(handGridClass(8)).toBe('grid-cols-8')
    expect(handGridClass(7)).toBe('grid-cols-7')
    expect(handGridClass(3)).toBe('grid-cols-7')
  })
})

describe('mulligan labels', () => {
  it('titles the opening hand, with the mulligans taken', () => {
    expect(openingHandTitle(0)).toBe('Opening hand')
    expect(openingHandTitle(1)).toBe('Opening hand (mulligan to 6)')
    expect(openingHandTitle(2)).toBe('Opening hand (mulligan to 5)')
  })

  it('shows each side that mulliganed in the meta strip', () => {
    expect(mulliganStrip(game(), 'Qorvath')).toEqual([])
    expect(mulliganStrip(game({ localMulligans: 1, opponentMulligans: 2 }), 'Qorvath')).toEqual([
      { label: 'You mull', value: '×1' },
      { label: 'Qorvath mull', value: '×2' },
    ])
    expect(mulliganStrip(game({ opponentMulligans: 1 }), 'Opponent')).toEqual([
      { label: 'Opponent mull', value: '×1' },
    ])
  })
})

describe('game meta strip', () => {
  it('writes the duration as minutes and seconds, or seconds alone', () => {
    expect(durationText(0, 754_000)).toBe('12m 34s')
    expect(durationText(0, 60_000)).toBe('1m 0s')
    expect(durationText(0, 45_900)).toBe('45s')
    expect(durationText(0, 0)).toBe('0s')
  })

  it('has no duration without both ends', () => {
    expect(durationText(0, null)).toBeNull()
    expect(durationText(null, 1000)).toBeNull()
  })

  it('shows turns only when known', () => {
    expect(turnsText(null)).toBeNull()
    expect(turnsText(0)).toBe('0')
    expect(turnsText(9)).toBe('9')
  })

  it('says play or draw, an unknown counting as the draw (0.x)', () => {
    expect(playDrawText(true)).toBe('On the play')
    expect(playDrawText(false)).toBe('On the draw')
    expect(playDrawText(null)).toBe('On the draw')
  })

  it('tiles a game as won, lost, or unknown when its result was never observed', () => {
    expect(gameTile(game({ result: 'win' }))).toEqual({ text: 'Win', tone: 'win' })
    expect(gameTile(game({ result: 'loss' }))).toEqual({ text: 'Loss', tone: 'loss' })
    expect(gameTile(game({ result: 'unknown' }))).toEqual({ text: 'Unknown', tone: 'unknown' })
  })
})

describe('sideboard changes', () => {
  it('counts every card moved and signs each change', () => {
    const changes = [
      { catalogId: 1, name: 'A', image: null, localImage: null, quantity: 2, kind: 'in' as const },
      { catalogId: 2, name: 'B', image: null, localImage: null, quantity: 1, kind: 'out' as const },
    ]
    expect(sideboardCount(changes)).toBe(3)
    expect(changeLabel(changes[0])).toBe('+2')
    expect(changeLabel(changes[1])).toBe('−1')
  })
})

describe('opponent reveals', () => {
  it('groups unknown types as Other and canonical types by their first match', () => {
    expect(revealType(null)).toBe('Other')
    expect(revealType('Artifact Creature - Golem')).toBe('Creature')
    expect(revealType('Legendary Land')).toBe('Land')
    expect(revealType('Kindred Instant')).toBe('Instant')
    expect(revealType('Conspiracy')).toBe('Conspiracy')
  })

  it('merges games by name, keeping the most copies seen and each game once', () => {
    const games = [
      game({ number: 1, opponentCardsSeen: [reveal('Bolt', { quantity: 1 }), reveal('Island', { quantity: 3 })] }),
      game({ number: 2, opponentCardsSeen: [reveal('Bolt', { quantity: 2 })] }),
    ]
    expect(aggregateReveals(games, 'all')).toEqual([
      { name: 'Bolt', image: null, localImage: null, typeLine: null, colors: [], games: [1, 2], quantity: 2 },
      { name: 'Island', image: null, localImage: null, typeLine: null, colors: [], games: [1], quantity: 3 },
    ])
  })

  it('keeps only the filtered game', () => {
    const games = [
      game({ number: 1, opponentCardsSeen: [reveal('Bolt'), reveal('Island')] }),
      game({ number: 2, opponentCardsSeen: [reveal('Bolt', { quantity: 2 })] }),
    ]
    expect(aggregateReveals(games, 2)).toEqual([
      { name: 'Bolt', image: null, localImage: null, typeLine: null, colors: [], games: [2], quantity: 2 },
    ])
    expect(aggregateReveals(games, 3)).toEqual([])
  })

  it('leaves the opponent commander out of the reveals (it has its own panel)', () => {
    const games = [
      game({ number: 1, opponentCardsSeen: [reveal('Qomnath', { catalogId: 7 }), reveal('Bolt', { catalogId: 8 })] }),
    ]
    expect(aggregateReveals(games, 'all', [{ catalogId: 7, name: 'Qomnath' }]).map((card) => card.name)).toEqual(['Bolt'])
    // Another printing of the commander is still the commander.
    expect(aggregateReveals(games, 'all', [{ catalogId: 70, name: 'Qomnath' }]).map((card) => card.name)).toEqual(['Bolt'])
    expect(aggregateReveals(games, 'all').map((card) => card.name)).toEqual(['Qomnath', 'Bolt'])
  })

  it('offers All and one filter per game only when there are several games', () => {
    expect(revealFilters([game({ number: 1 })])).toEqual([])
    expect(revealFilters([game({ number: 1 }), game({ number: 2 }), game({ number: 3 })])).toEqual(['all', 1, 2, 3])
  })

  it('orders groups as 0.x did, others last, with cards by name and a copy count', () => {
    const games = [
      game({
        number: 1,
        opponentCardsSeen: [
          reveal('Mountain', { typeLine: 'Basic Land - Mountain', quantity: 2 }),
          reveal('Scheme', { typeLine: 'Scheme' }),
          reveal('Shock', { typeLine: 'Instant' }),
          reveal('Ragavan', { typeLine: 'Legendary Creature - Monkey Pirate' }),
          reveal('Bolt', { typeLine: 'Instant', quantity: 3 }),
          reveal('Mystery'),
          reveal('Karn', { typeLine: 'Legendary Planeswalker - Karn' }),
          reveal('Bauble', { typeLine: 'Artifact' }),
        ],
      }),
    ]
    const groups = groupReveals(aggregateReveals(games, 'all'))
    expect(groups.map((g) => [g.type, g.count, g.cards.map((c) => c.name)])).toEqual([
      ['Creature', 1, ['Ragavan']],
      ['Planeswalker', 1, ['Karn']],
      ['Instant', 4, ['Bolt', 'Shock']],
      ['Artifact', 1, ['Bauble']],
      ['Land', 2, ['Mountain']],
      ['Scheme', 1, ['Scheme']],
      ['Other', 1, ['Mystery']],
    ])
  })

  it('borders each card with its colours', () => {
    expect(revealBorderStyle([])).toEqual({
      borderLeftColor: 'transparent',
      borderLeftWidth: '3px',
      borderLeftStyle: 'solid',
    })
    expect(revealBorderStyle(['R'])).toEqual({
      borderLeftColor: '#E49977',
      borderLeftWidth: '3px',
      borderLeftStyle: 'solid',
    })
    expect(revealBorderStyle(['U', 'R'])).toEqual({
      borderImage: 'linear-gradient(to bottom, #C1D7E9 0% 50%, #E49977 50% 100%) 1',
      borderLeftWidth: '3px',
      borderLeftStyle: 'solid',
    })
    expect(revealBorderStyle(['C'])).toEqual(revealBorderStyle([]))
  })

  it('keeps the hover preview inside the window', () => {
    expect(previewTop(100, 900)).toBe(100)
    expect(previewTop(800, 900)).toBe(620)
    expect(previewTop(-50, 900)).toBe(8)
  })
})

describe('match hero', () => {
  it('formats the start as the matches table does (month by name, local time)', () => {
    expect(startedAtText(new Date(2026, 8, 7, 15, 5).getTime())).toBe('7 Sep 2026, 3:05pm')
    expect(startedAtText(new Date(2026, 0, 12, 0, 30).getTime())).toBe('12 Jan 2026, 12:30am')
    expect(startedAtText(new Date(2026, 0, 12, 12, 0).getTime())).toBe('12 Jan 2026, 12:00pm')
  })

  it('writes the match time as the largest whole unit', () => {
    expect(matchTimeText(0, 23 * 60_000 + 5_000)).toBe('23 minutes')
    expect(matchTimeText(0, 3_600_000)).toBe('1 hour')
    expect(matchTimeText(0, null)).toBeNull()
  })

  it('names the result by the outcome, else by the games', () => {
    expect(heroResult(detail({ outcome: 'win', wins: 0, losses: 0 }))).toEqual({ text: 'Win', win: true })
    expect(heroResult(detail({ outcome: 'loss', wins: 2, losses: 1 }))).toEqual({ text: 'Loss', win: false })
    expect(heroResult(detail({ outcome: 'unknown', wins: 2, losses: 1 }))).toEqual({ text: 'Win', win: true })
    expect(heroResult(detail({ outcome: 'unknown', wins: 1, losses: 1 }))).toEqual({ text: 'Loss', win: false })
  })

  it('writes the score and the games played', () => {
    expect(scoreText(2, 1)).toBe('2–1')
    expect(gamesCountText(1)).toBe('1 game')
    expect(gamesCountText(3)).toBe('3 games')
    expect(gamesCountText(0)).toBe('0 games')
  })

  it('falls back to the deck matches page, else every match', () => {
    expect(matchesFallbackPath({ id: 7, name: 'Q', colors: [], versionId: 2 })).toBe('/decks/7/matches')
    expect(matchesFallbackPath(null)).toBe('/matches')
  })
})

describe('notes', () => {
  it('allows up to 5000 characters', () => {
    expect(NOTES_MAX).toBe(5000)
    expect(notesTooLong('a'.repeat(5000))).toBe(false)
    expect(notesTooLong('a'.repeat(5001))).toBe(true)
  })
})

describe('refresh pushes', () => {
  /** Fake event sources that keep each listener, so a test can fire it. */
  function fakeEvents() {
    const listeners: Record<string, (event: { payload: unknown }) => void> = {}
    let stopped = 0
    function source(name: string) {
      return {
        listen: (cb: (event: { payload: never }) => void) => {
          listeners[name] = cb as (event: { payload: unknown }) => void
          return Promise.resolve(() => {
            stopped += 1
          })
        },
      }
    }
    const events: DetailEventSource = {
      matchUpdated: source('matchUpdated'),
      matchRemoved: source('matchRemoved'),
      decksChanged: source('decksChanged'),
      leaguesChanged: source('leaguesChanged'),
    }
    return { events, fire: (name: string, payload: unknown) => listeners[name]({ payload }), stopped: () => stopped }
  }

  it("refreshes on this match's update or removal and on any deck or league change", async () => {
    const { events, fire, stopped } = fakeEvents()
    let refreshes = 0
    const stop = await listenForDetailChanges(events, () => 'q-token', () => {
      refreshes += 1
    })

    fire('matchUpdated', { summary: { token: 'q-other' } })
    fire('matchRemoved', { token: 'q-other' })
    expect(refreshes).toBe(0)

    fire('matchUpdated', { summary: { token: 'q-token' } })
    fire('matchRemoved', { token: 'q-token' })
    fire('decksChanged', {})
    // The hero shows the league's name.
    fire('leaguesChanged', {})
    expect(refreshes).toBe(4)

    stop()
    expect(stopped()).toBe(4)
  })
})

describe('ratingText', () => {
  it('reads before to after with the signed change', () => {
    expect(ratingText({ before: 1371, after: 1336, delta: -35 })).toBe('Rating 1371 to 1336')
    expect(ratingText({ before: 1336, after: 1348, delta: 12 })).toBe('Rating 1336 to 1348')
    expect(ratingText({ before: 1336, after: 1336, delta: 0 })).toBe('Rating 1336 to 1336')
  })
})
