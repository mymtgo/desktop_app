import { describe, expect, it } from 'vitest'
import type { DeckCardOptionView, GameDetailView, ManualLeagueOption, MatchEditView } from '@/bindings'
import {
  addRevealCard,
  bumpReveal,
  bumpSideboard,
  canSubmitEdit,
  canSubmitMatch,
  correctionPayload,
  editFormFromView,
  emptyGames,
  gamesForKind,
  endedAtFor,
  errorsBesides,
  firstError,
  gameRowsFromView,
  gamesPayload,
  handHint,
  handPayload,
  handStateFromGame,
  isGameEnabled,
  keepLeague,
  leagueOptionsFor,
  plusMinutes,
  rankGapFor,
  removeFromHand,
  revealCopySources,
  revealRowsFromGame,
  revealsPayload,
  searchTerm,
  setMulligans,
  setResult,
  sideboardPayload,
  sideboardStateFromGame,
  toLocalInput,
  toggleBottomed,
  turnsError,
  type GameRow,
} from './manualMatch'

function rows(...results: GameRow['result'][]): GameRow[] {
  const games = emptyGames()
  results.forEach((result, i) => {
    games[i].result = result
  })
  return games
}

function game(overrides: Partial<GameDetailView> = {}): GameDetailView {
  return {
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
    ...overrides,
  } as GameDetailView
}

function card(catalogId: number, name: string, quantity = 4): DeckCardOptionView {
  return { catalogId, name, image: null, localImage: null, quantity }
}

describe('game rows', () => {
  it('start with three empty rows, game 2 on the draw', () => {
    expect(emptyGames()).toEqual([
      { result: '', onPlay: 'play', turns: '' },
      { result: '', onPlay: 'draw', turns: '' },
      { result: '', onPlay: 'play', turns: '' },
    ])
  })

  it('unlock in order', () => {
    const games = emptyGames()
    expect(isGameEnabled(games, 0)).toBe(true)
    expect(isGameEnabled(games, 1)).toBe(false)
    expect(isGameEnabled(games, 2)).toBe(false)

    const one = rows('W')
    expect(isGameEnabled(one, 1)).toBe(true)
    expect(isGameEnabled(one, 2)).toBe(false)
  })

  it('stop at two wins or two losses', () => {
    expect(isGameEnabled(rows('W', 'W'), 2)).toBe(false)
    expect(isGameEnabled(rows('L', 'L'), 2)).toBe(false)
    expect(isGameEnabled(rows('W', 'L'), 2)).toBe(true)
  })

  it('a locked row cannot take a result', () => {
    const games = setResult(emptyGames(), 2, 'W')
    expect(games[2].result).toBe('')
  })

  it('clicking a result again clears it', () => {
    const won = setResult(emptyGames(), 0, 'W')
    expect(won[0].result).toBe('W')
    expect(setResult(won, 0, 'W')[0].result).toBe('')
    expect(setResult(won, 0, 'L')[0].result).toBe('L')
  })

  it('clearing a result clears the rows that no longer unlock', () => {
    const full = rows('W', 'L', 'W')
    const cleared = setResult(full, 0, 'W')
    expect(cleared.map((g) => g.result)).toEqual(['', '', ''])

    // Game 2 turning a 1-1 into a 2-0 locks game 3.
    const twoNil = setResult(full, 1, 'W')
    expect(twoNil.map((g) => g.result)).toEqual(['W', 'W', ''])
  })

  it('leaves the rows it was given alone', () => {
    const games = emptyGames()
    setResult(games, 0, 'W')
    expect(games[0].result).toBe('')
  })

  it('a 1-1 after two games is a draw that can be saved as is', () => {
    const games = rows('W', 'L')
    expect(isGameEnabled(games, 2)).toBe(true)
    expect(gamesPayload(games)).toEqual([
      { won: true, onPlay: true, turns: null },
      { won: false, onPlay: false, turns: null },
    ])
    expect(canSubmitMatch({ deckId: 1, opponentName: 'Qorvath', games, saving: false, opponentRequired: true })).toBe(true)
  })

  it('send only the games with a result, with turns when given', () => {
    const games = rows('L', 'W', 'W')
    games[0].turns = 7
    games[1].turns = ' 12 '
    games[2].turns = ''
    games[2].onPlay = 'draw'
    expect(gamesPayload(games)).toEqual([
      { won: false, onPlay: true, turns: 7 },
      { won: true, onPlay: false, turns: 12 },
      { won: true, onPlay: false, turns: null },
    ])
  })

  it('need a deck, an opponent and a game to submit', () => {
    const games = rows('W')
    expect(canSubmitMatch({ deckId: null, opponentName: 'Qorvath', games, saving: false, opponentRequired: true })).toBe(false)
    expect(canSubmitMatch({ deckId: 1, opponentName: '  ', games, saving: false, opponentRequired: true })).toBe(false)
    expect(canSubmitMatch({ deckId: 1, opponentName: 'Qorvath', games: emptyGames(), saving: false, opponentRequired: true })).toBe(false)
    expect(canSubmitMatch({ deckId: 1, opponentName: 'Qorvath', games, saving: true, opponentRequired: true })).toBe(false)
    expect(canSubmitMatch({ deckId: 1, opponentName: 'Qorvath', games, saving: false, opponentRequired: true })).toBe(true)
  })

  it('an arena match can be submitted without an opponent', () => {
    const games = rows('W')
    expect(canSubmitMatch({ deckId: 1, opponentName: '  ', games, saving: false, opponentRequired: false })).toBe(true)
    expect(canSubmitMatch({ deckId: 1, opponentName: '', games: emptyGames(), saving: false, opponentRequired: false })).toBe(
      false,
    )
    expect(canSubmitMatch({ deckId: null, opponentName: '', games, saving: false, opponentRequired: false })).toBe(false)
  })
})

describe('started and ended', () => {
  it('format a local datetime-local value', () => {
    expect(toLocalInput(new Date(2026, 9, 8, 7, 5))).toBe('2026-10-08T07:05')
  })

  it('add minutes across the hour and the day', () => {
    expect(plusMinutes('2026-10-08T19:30', 45)).toBe('2026-10-08T20:15')
    expect(plusMinutes('2026-10-08T23:30', 45)).toBe('2026-10-09T00:15')
    expect(plusMinutes('', 45)).toBe('')
    expect(plusMinutes('nonsense', 45)).toBe('')
  })

  it('Ended follows Started plus 45 minutes until it is edited', () => {
    expect(endedAtFor('2026-10-08T19:30', '2026-10-08T19:00', false)).toBe('2026-10-08T20:15')
    expect(endedAtFor('2026-10-08T19:30', '2026-10-08T22:00', true)).toBe('2026-10-08T22:00')
  })
})

describe('edit match', () => {
  const view = (
    games: MatchEditView['games'],
    ended: number | null = null,
    started: number = new Date(2026, 9, 16, 20, 5).getTime(),
  ): MatchEditView => ({
    opponentName: 'Qrival',
    startedAtMs: started,
    endedAtMs: ended,
    games,
    corrected: false,
    platform: 'mtgo',
  })

  it('fills the rows from the stored games and keeps the empty third row', () => {
    const rows = gameRowsFromView([
      { gameNumber: 1, result: 'win', onPlay: false, turns: 7 },
      { gameNumber: 2, result: 'loss', onPlay: null, turns: null },
    ])
    expect(rows).toEqual([
      { result: 'W', onPlay: 'draw', turns: 7 },
      { result: 'L', onPlay: 'draw', turns: '' },
      { result: '', onPlay: 'play', turns: '' },
    ])
  })

  it('starts with the empty rows for a match known only by its counts', () => {
    expect(gameRowsFromView([])).toEqual(emptyGames())
  })

  it('leaves a drawn or unknown game, and the games after it, without a result', () => {
    const rows = gameRowsFromView([
      { gameNumber: 1, result: 'draw', onPlay: true, turns: null },
      { gameNumber: 2, result: 'win', onPlay: false, turns: null },
    ])
    expect(rows.map((r) => r.result)).toEqual(['', '', ''])
    expect(rows[1].onPlay).toBe('draw')
  })

  it('opens with the local start and end, and no end when the match has none', () => {
    const ended = new Date(2026, 9, 16, 20, 50).getTime()
    expect(editFormFromView(view([], ended))).toMatchObject({
      opponentName: 'Qrival',
      startedAt: '2026-10-16T20:05',
      endedAt: '2026-10-16T20:50',
    })
    expect(editFormFromView({ ...view([]), opponentName: null })).toMatchObject({ opponentName: '', endedAt: '' })
  })

  it('sends a time the player changed, and a blank end, a kept time and the games with a result', () => {
    const games = setResult(emptyGames(), 0, 'W')
    const opened = { startedAt: '2026-10-16T20:05', endedAt: '2026-10-16T20:50' }
    expect(
      correctionPayload({ opponentName: ' Qrival ', startedAt: '2026-10-16T20:10', endedAt: ' ', games }, opened),
    ).toEqual({
      opponentName: 'Qrival',
      startedAt: '2026-10-16T20:10',
      endedAt: null,
      games: [{ won: true, onPlay: true, turns: null }],
    })
    expect(
      correctionPayload({ opponentName: 'Qrival', startedAt: '2026-10-16T20:05', endedAt: '2026-10-16T21:00', games }, opened),
    ).toMatchObject({ startedAt: null, endedAt: '2026-10-16T21:00' })
  })

  it('sends both times as null on an outcome-only Save, so the stored seconds stay', () => {
    // The inputs show minutes: 20:05:42 opens as 20:05, and sending that
    // back would move the match 42 seconds.
    const started = new Date(2026, 9, 16, 20, 5, 42).getTime()
    const ended = new Date(2026, 9, 16, 20, 50, 17).getTime()
    const form = editFormFromView(view([], ended, started))
    const won = { ...form, games: setResult(form.games, 0, 'W') }
    expect(correctionPayload(won, form)).toMatchObject({ startedAt: null, endedAt: null })
  })

  it('needs an opponent and a game, as Add does, but no deck', () => {
    const games = setResult(emptyGames(), 0, 'L')
    const mtgo = { games, saving: false, opponentRequired: true }
    expect(canSubmitEdit({ ...mtgo, opponentName: 'Qrival' })).toBe(true)
    expect(canSubmitEdit({ ...mtgo, opponentName: ' ' })).toBe(false)
    expect(canSubmitEdit({ ...mtgo, opponentName: 'Qrival', games: emptyGames() })).toBe(false)
    expect(canSubmitEdit({ ...mtgo, opponentName: 'Qrival', saving: true })).toBe(false)
  })

  it('lets an arena edit leave the opponent blank', () => {
    const games = setResult(emptyGames(), 0, 'L')
    expect(canSubmitEdit({ opponentName: ' ', games, saving: false, opponentRequired: false })).toBe(true)
  })
})

describe('league options', () => {
  const options: ManualLeagueOption[] = [
    { key: 'QTOKEN:1', name: 'Modern League', deckId: 3, matchCount: 2, roundCount: 5, startedAtMs: 3 },
    { key: 'manual:1', name: 'My league', deckId: null, matchCount: 0, roundCount: 5, startedAtMs: 2 },
    { key: 'QTOKEN:2', name: 'Other deck', deckId: 4, matchCount: 1, roundCount: 5, startedAtMs: 1 },
    { key: 'manual:2', name: 'Full', deckId: 3, matchCount: 5, roundCount: 5, startedAtMs: 0 },
  ]

  it('offer open leagues of the deck or of no deck (0.x `leagueOptions`)', () => {
    expect(leagueOptionsFor(options, 3).map((o) => o.key)).toEqual(['QTOKEN:1', 'manual:1'])
    expect(leagueOptionsFor(options, null).map((o) => o.key)).toEqual(['QTOKEN:1', 'manual:1', 'QTOKEN:2'])
  })

  it('keep a chosen league only while it is offered', () => {
    expect(keepLeague('manual:1', options)).toBe('manual:1')
    expect(keepLeague('gone', options)).toBeNull()
    expect(keepLeague(null, options)).toBeNull()
  })
})

describe('rank gap', () => {
  it('reads the select value whatever the options, none as null', () => {
    expect(rankGapFor('1:500')).toEqual({ fileId: 1, byteOffset: 500 })
    expect(rankGapFor('none')).toBeNull()
    expect(rankGapFor('1:2:3')).toBeNull()
  })
})

describe('field errors', () => {
  it('find the turns error and the first error', () => {
    const errors = { 'games.1.turns': 'The games.1.turns field must not be greater than 99.', opponent_name: 'Required.' }
    expect(turnsError(errors)).toBe('The games.1.turns field must not be greater than 99.')
    expect(turnsError({})).toBeNull()
    expect(firstError({})).toBeNull()
    expect(firstError({ kept_hand: 'The kept hand must have 7 cards.' })).toBe('The kept hand must have 7 cards.')
  })

  it('list the errors no field shows', () => {
    const errors = { deck_id: 'The selected deck id is invalid.', opponent_name: 'Required.' }
    expect(errorsBesides(errors, ['opponent_name'])).toEqual(['The selected deck id is invalid.'])
  })
})

describe('opening hand', () => {
  it('starts from the recorded hand, its bottomed cards marked', () => {
    const state = handStateFromGame(
      game({
        localMulligans: 1,
        openingHand: [1, 1, 2, 2, 3, 3, 3].map((catalogId, i) => ({
          catalogId,
          name: '',
          image: null,
          localImage: null,
          bottomed: i === 6,
          firstDraw: false,
        })),
        mulliganedHands: [],
      }),
    )
    expect(state.mulligans).toBe(1)
    expect(state.hands).toEqual([[], [1, 1, 2, 2, 3, 3, 3]])
    expect([...state.bottomed]).toEqual([6])
  })

  it('builds the payload: the starting hand, bottomed cards, put-back hands', () => {
    const state = {
      mulligans: 2,
      hands: [[1, 1, 1, 1, 2, 2, 2], [], [1, 2, 3, 1, 2, 3, 1]],
      bottomed: new Set([1, 4]),
    }
    expect(handPayload(state)).toEqual({
      mulliganCount: 2,
      startingHand: [1, 3, 1, 3, 1],
      bottomed: [2, 2],
      mulliganedHands: [[1, 1, 1, 1, 2, 2, 2], []],
    })
  })

  it('says what is missing before it can be saved', () => {
    const seven = [1, 1, 2, 2, 3, 3, 3]
    expect(handHint({ mulligans: 0, hands: [[1]], bottomed: new Set() })).toBe('Final hand needs 7 cards.')
    expect(handHint({ mulligans: 1, hands: [[], seven], bottomed: new Set() })).toBe('Mark the card you bottomed.')
    expect(handHint({ mulligans: 2, hands: [[], [], seven], bottomed: new Set([0]) })).toBe(
      'Mark the 2 cards you bottomed.',
    )
    expect(handHint({ mulligans: 1, hands: [[1], seven], bottomed: new Set([0]) })).toBe(
      'A put-back hand is seven cards, or leave it empty if you do not remember it.',
    )
    expect(handHint({ mulligans: 1, hands: [[], seven], bottomed: new Set([0]) })).toBeNull()
  })

  it('keeps the final hand when the mulligans change, and trims the bottomed marks', () => {
    const seven = [1, 1, 2, 2, 3, 3, 3]
    const two = setMulligans({ mulligans: 0, hands: [seven], bottomed: new Set() }, 2)
    expect(two.hands).toEqual([[], [], seven])
    const marked = { ...two, bottomed: new Set([0, 1]) }
    const one = setMulligans(marked, 1)
    expect(one.hands).toEqual([[], seven])
    expect([...one.bottomed]).toEqual([0])
    expect(setMulligans(one, 9).mulligans).toBe(6)
    expect(setMulligans(one, -1).mulligans).toBe(0)
  })

  it('shifts the bottomed marks when a final hand card is removed', () => {
    const state = { mulligans: 2, hands: [[], [], [1, 2, 3, 1, 2, 3, 1]], bottomed: new Set([1, 4]) }
    const removed = removeFromHand(state, 2, 2)
    expect(removed.hands[2]).toEqual([1, 2, 1, 2, 3, 1])
    expect([...removed.bottomed].sort()).toEqual([1, 3])
  })

  it('marks no more bottomed cards than mulligans', () => {
    const state = { mulligans: 1, hands: [[], [1, 2, 3, 1, 2, 3, 1]], bottomed: new Set<number>() }
    const one = toggleBottomed(state, 2)
    expect([...one.bottomed]).toEqual([2])
    expect([...toggleBottomed(one, 3).bottomed]).toEqual([2])
    expect([...toggleBottomed(one, 2).bottomed]).toEqual([])
  })
})

describe('sideboard', () => {
  it('starts from the recorded changes', () => {
    const state = sideboardStateFromGame([
      { catalogId: 1, name: 'Qelm', image: null, localImage: null, quantity: 2, kind: 'out' },
      { catalogId: 9, name: 'Qwall', image: null, localImage: null, quantity: 2, kind: 'in' },
    ])
    expect([...state.outs]).toEqual([[1, 2]])
    expect([...state.ins]).toEqual([[9, 2]])
  })

  it('bumps within 0 and the copies the deck has', () => {
    const qelm = card(1, 'Qelm', 2)
    let outs = new Map<number, number>()
    outs = bumpSideboard(outs, qelm, 1)
    outs = bumpSideboard(outs, qelm, 1)
    outs = bumpSideboard(outs, qelm, 1)
    expect(outs.get(1)).toBe(2)
    outs = bumpSideboard(outs, qelm, -1)
    outs = bumpSideboard(outs, qelm, -1)
    expect(outs.has(1)).toBe(false)
  })

  it('builds the payload, outs first', () => {
    expect(
      sideboardPayload({
        outs: new Map([[1, 2]]),
        ins: new Map([
          [9, 1],
          [8, 1],
        ]),
      }),
    ).toEqual([
      { catalogId: 1, quantity: 2, kind: 'out' },
      { catalogId: 9, quantity: 1, kind: 'in' },
      { catalogId: 8, quantity: 1, kind: 'in' },
    ])
  })
})

describe('reveals', () => {
  const seen = (catalogId: number, name: string, quantity: number) => ({
    catalogId,
    name,
    image: null,
    localImage: null,
    typeLine: null,
    colors: [],
    quantity,
  })

  it('starts from the recorded cards', () => {
    expect(revealRowsFromGame(game({ opponentCardsSeen: [seen(5, 'Qbolt', 2)] }))).toEqual([
      { catalogId: 5, name: 'Qbolt', quantity: 2 },
    ])
  })

  it('adds a card once, then adds copies up to 20', () => {
    let rows = addRevealCard([], { catalogId: 5, name: 'Qbolt' })
    rows = addRevealCard(rows, { catalogId: 5, name: 'Qbolt' })
    expect(rows).toEqual([{ catalogId: 5, name: 'Qbolt', quantity: 2 }])
    for (let i = 0; i < 30; i++) rows = addRevealCard(rows, { catalogId: 5, name: 'Qbolt' })
    expect(rows[0].quantity).toBe(20)
  })

  it('bumps between 1 and 20', () => {
    const row = { catalogId: 5, name: 'Qbolt', quantity: 1 }
    expect(bumpReveal([row], 0, -1)[0].quantity).toBe(1)
    expect(bumpReveal([{ ...row, quantity: 20 }], 0, 1)[0].quantity).toBe(20)
    expect(bumpReveal([row], 0, 1)[0].quantity).toBe(2)
  })

  it('copies from another game that has cards', () => {
    const games = [
      game({ number: 1, opponentCardsSeen: [seen(5, 'Qbolt', 1)] }),
      game({ number: 2 }),
      game({ number: 3, opponentCardsSeen: [seen(6, 'Qhelix', 1)] }),
    ]
    expect(revealCopySources(games, 3).map((g) => g.number)).toEqual([1])
    expect(revealCopySources(games, 2).map((g) => g.number)).toEqual([1, 3])
  })

  it('builds the payload', () => {
    expect(revealsPayload([{ catalogId: 5, name: 'Qbolt', quantity: 2 }])).toEqual([{ catalogId: 5, quantity: 2 }])
  })
})

describe('card search', () => {
  it('searches from two characters, trimmed', () => {
    expect(searchTerm(' q ')).toBeNull()
    expect(searchTerm(' qb ')).toBe('qb')
  })
})

describe('Bo1 game rows', () => {
  it('keep only the first row', () => {
    const rows = setResult(setResult(emptyGames(), 0, 'W'), 1, 'L')
    expect(gamesForKind(rows, 'bo1')).toHaveLength(1)
    expect(gamesForKind(rows, 'bo1')[0].result).toBe('W')
    expect(gamesForKind(rows, 'bo3')).toHaveLength(3)
  })

  it('come back as three rows when Bo3 is chosen again', () => {
    const one = gamesForKind(emptyGames(), 'bo1')
    expect(gamesForKind(one, 'bo3')).toHaveLength(3)
    expect(gamesForKind(one, 'bo3')[1].onPlay).toBe('draw')
  })

  it('take back the set aside rows when Bo3 is chosen again', () => {
    const rows = setResult(setResult(emptyGames(), 0, 'W'), 1, 'L')
    const one = gamesForKind(rows, 'bo1')
    const back = gamesForKind(one, 'bo3', rows)
    expect(back.map((g) => g.result)).toEqual(['W', 'L', ''])
    expect(back[1].onPlay).toBe('draw')
  })

  it('clear a taken back row that no longer unlocks', () => {
    const rows = setResult(setResult(emptyGames(), 0, 'W'), 1, 'W')
    // Game 1 became a loss while Bo1: W, W was a finished 2-0.
    const one = setResult(gamesForKind(rows, 'bo1'), 0, 'L')
    expect(gamesForKind(one, 'bo3', rows).map((g) => g.result)).toEqual(['L', 'W', ''])
    const won = setResult(setResult(setResult(emptyGames(), 0, 'W'), 1, 'L'), 2, 'W')
    const lost = setResult(gamesForKind(won, 'bo1'), 0, 'L')
    // L, L ends the match: game 3 is locked again and loses its result.
    expect(gamesForKind(lost, 'bo3', won).map((g) => g.result)).toEqual(['L', 'L', ''])
  })

  it('make one game a finished match', () => {
    const one = gamesForKind(emptyGames(), 'bo1')
    expect(isGameEnabled(one, 0)).toBe(true)
  })
})
