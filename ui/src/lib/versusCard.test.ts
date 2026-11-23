import { describe, expect, it } from 'vitest'
import type { GamePip, OverlaySize } from '@/bindings'
import type { LeagueDisplay, LeagueOverlayState, LeagueStatus } from '@/bindings'
import { sampleEventState, sampleOverlayState, TOURNAMENT_ROWS } from '@/components/overlay/overlaySamples'
import { DEFAULT_DISPLAY, nextGameNumber } from '@/lib/leagueOverlayCard'
import {
  bars,
  bottomLine,
  bottomLines,
  callout,
  damageChunks,
  compactMiddle,
  introText,
  matchGold,
  subtitleLadder,
  type Callout,
  type RecordLine,
  isLow,
  lifeShare,
  matchText,
  nameFit,
  opponentName,
  nextLastMatch,
  roundPips,
  sideboardBanner,
  versusAnimations,
  yourName,
} from './versusCard'

const NOW = 1_800_000_000_000

function state(status: LeagueStatus, change: Partial<LeagueOverlayState> = {}): LeagueOverlayState {
  return { ...sampleOverlayState(status, { art: null, size: 'full', theme: 'versus' }), ...change }
}

function inGame(games: GamePip[], number = 5, life = { you: 14, opponent: 4, startYou: 20, startOpponent: 20 }) {
  const won = games.filter((game) => game.won === true).length
  const lost = games.filter((game) => game.won === false).length
  return state('in_game', { match: { number, games, gamesWon: won, gamesLost: lost, life } })
}

function sideboarding(games: GamePip[], number = 5) {
  const won = games.filter((game) => game.won === true).length
  const lost = games.filter((game) => game.won === false).length
  return state('sideboarding', { match: { number, games, gamesWon: won, gamesLost: lost, life: null } })
}

describe('versus bars', () => {
  it('clamps the bar share', () => {
    expect(lifeShare(14, 20)).toBe(0.7)
    expect(lifeShare(27, 20)).toBe(1)
    expect(lifeShare(-3, 20)).toBe(0)
    expect(lifeShare(5, 0)).toBe(0)
    expect(lifeShare(0, 0)).toBe(0)
  })

  it('flashes at 4 life or less', () => {
    expect(isLow(4)).toBe(true)
    expect(isLow(5)).toBe(false)
  })

  it('reads the live life in game and full bars otherwise', () => {
    expect(bars(inGame([{ won: true }, { won: false }, { won: null }]), null)).toEqual({
      you: 0.7, opponent: 0.2, youLife: 14, opponentLife: 4,
    })
    expect(bars(sideboarding([{ won: true }]), { you: 20, opponent: 20 })).toEqual({
      you: 1, opponent: 1, youLife: 20, opponentLife: 20,
    })
    expect(bars(state('waiting'), null)).toEqual({ you: 1, opponent: 1, youLife: null, opponentLife: null })
  })

  it('shows an empty bar for a starting life of 0', () => {
    const zero = inGame([{ won: null }], 5, { you: 0, opponent: 3, startYou: 0, startOpponent: 0 })
    expect(bars(zero, null)).toEqual({ you: 0, opponent: 0, youLife: 0, opponentLife: 3 })
  })

  it('leaves a damage chunk for each side that dropped', () => {
    const before = { you: 1, opponent: 0.8, youLife: 20, opponentLife: 16 }
    expect(damageChunks(before, { you: 0.7, opponent: 0.5, youLife: 14, opponentLife: 10 })).toEqual([
      { side: 'you', from: 1, to: 0.7 },
      { side: 'opponent', from: 0.8, to: 0.5 },
    ])
    expect(damageChunks(before, { you: 1, opponent: 0.6, youLife: 20, opponentLife: 12 })).toEqual([
      { side: 'opponent', from: 0.8, to: 0.6 },
    ])
    expect(damageChunks(before, { you: 1, opponent: 1, youLife: 20, opponentLife: 20 })).toEqual([])
    expect(damageChunks(null, before)).toEqual([])
  })
})

describe('versus text', () => {
  it('counts games won as pips with two slots', () => {
    expect(roundPips(inGame([{ won: true }, { won: false }, { won: null }]))).toEqual({
      you: [true, false], opponent: [true, false],
    })
    expect(roundPips(state('waiting'))).toEqual({ you: [false, false], opponent: [false, false] })
  })

  it('names you by archetype, or YOU with Archetype off', () => {
    expect(yourName(inGame([]))).toBe('Mono Green Tron')
    expect(yourName({ ...inGame([]), display: { ...DEFAULT_DISPLAY, title: false } })).toBe('YOU')
    expect(yourName(inGame([]), 'My Spicy Sultai Brew')).toBe('My Spicy Sultai Brew')
    expect(yourName(inGame([]), '  ')).toBe('Mono Green Tron')
    expect(yourName({ ...inGame([]), display: { ...DEFAULT_DISPLAY, title: false } }, 'My Brew')).toBe('YOU')
  })

  it('names the opponent by their archetype, CHALLENGER when none is known', () => {
    const withArchetype = (opponentArchetype: string | null | undefined) => {
      const base = inGame([])
      return { ...base, match: { ...base.match!, opponentArchetype } }
    }
    expect(opponentName(inGame([]))).toBe('CHALLENGER')
    expect(opponentName(withArchetype(undefined))).toBe('CHALLENGER')
    expect(opponentName(withArchetype(null))).toBe('CHALLENGER')
    expect(opponentName(withArchetype('  '))).toBe('CHALLENGER')
    expect(opponentName(withArchetype('Dimir Tempo'))).toBe('Dimir Tempo')
    expect(opponentName(withArchetype(' Dimir Tempo '))).toBe('Dimir Tempo')
    expect(opponentName(state('waiting'))).toBe('CHALLENGER')
  })

  it('shows the match and the bottom line by the switches', () => {
    expect(matchText(inGame([]), false)).toBe('MATCH 5/5')
    expect(matchText(inGame([]), true)).toBe('M5/5')
    expect(matchText(state('waiting'), false)).toBe('MATCH 5/5')
    expect(matchText({ ...inGame([]), display: { ...DEFAULT_DISPLAY, progress: false } }, false)).toBeNull()
    expect(bottomLine(inGame([]))).toEqual({ lead: 'MODERN', record: '3-1' })
    expect(bottomLine({ ...inGame([]), display: { ...DEFAULT_DISPLAY, record: false } })).toEqual({
      lead: 'MODERN',
      record: null,
    })
    expect(bottomLine({ ...inGame([]), display: { ...DEFAULT_DISPLAY, event: false, format: false } })).toEqual({
      lead: null,
      record: '3-1',
    })
    expect(bottomLine({ ...inGame([]), display: { ...DEFAULT_DISPLAY, event: false, format: false, record: false } })).toBeNull()
  })

  it('sizes the names down as they grow, keeping typical archetypes whole', () => {
    expect(nameFit('CHALLENGER')).toBe('normal')
    expect(nameFit('Boros Energy')).toBe('normal')
    expect(nameFit('Izzet Prowess')).toBe('normal')
    expect(nameFit('Mono Green Tron')).toBe('long')
    expect(nameFit('Golgari Yawgmoth')).toBe('long')
    expect(nameFit('Dimir Murktide Control')).toBe('longer')
  })

  it('shows the compact middle: match and record, or the round banner', () => {
    const off = (change: Partial<typeof DEFAULT_DISPLAY>) => ({ ...inGame([]), display: { ...DEFAULT_DISPLAY, ...change } })
    expect(compactMiddle(inGame([]))).toEqual({ lead: 'M5/5', record: '3-1' })
    expect(compactMiddle(off({ record: false }))).toEqual({ lead: 'M5/5', record: null })
    expect(compactMiddle(off({ progress: false }))).toEqual({ lead: null, record: '3-1' })
    expect(compactMiddle(off({ record: false, progress: false }))).toBeNull()
    expect(compactMiddle(sideboarding([{ won: true }]))).toEqual({ lead: 'GAME 2', record: null })
  })

  it('calls out each state', () => {
    expect(callout(state('waiting'), NOW)).toMatchObject({ title: 'A CHALLENGER APPROACHES', subtitle: 'LOOKING FOR AN OPPONENT · MATCH 5/5 · 3-1', pulse: true })
    expect(callout(state('trophied'), NOW)?.title).toBe('PERFECT')
    expect(callout(state('trophied'), NOW)?.subtitle).toBe('MODERN · 5-0 · 10-2 IN GAMES')
    expect(callout(state('complete'), NOW)?.title).toBe('YOU WIN')
    expect(callout(state('complete', { record: { wins: 2, losses: 3 } }), NOW)?.title).toBe('CONTINUE?')
    expect(callout(state('dropped'), NOW)).toMatchObject({ title: 'GAME OVER', subtitle: 'DROPPED AFTER MATCH 3 · 1-2' })
    expect(callout(state('idle'), NOW)).toMatchObject({ title: 'INSERT COIN', subtitle: 'JOIN AN EVENT ON MTGO' })
    expect(callout(inGame([]), NOW)).toBeNull()
    expect(callout({ ...state('waiting'), display: { ...DEFAULT_DISPLAY, status: false } }, NOW)).toBeNull()
  })

  it('bans the next game while sideboarding', () => {
    expect(sideboardBanner(sideboarding([{ won: true }]))).toBe('GAME 2 · SIDEBOARDING')
    expect(sideboardBanner(inGame([]))).toBeNull()
  })
})

describe('versus animations', () => {
  it('plays nothing for the first state', () => {
    expect(versusAnimations(null, inGame([{ won: null }]), null)).toEqual([])
  })

  it('fights at the start of each game', () => {
    expect(versusAnimations(state('waiting'), inGame([{ won: null }]), null)).toEqual([{ kind: 'fight', round: 1 }])
    expect(
      versusAnimations(sideboarding([{ won: true }]), inGame([{ won: true }, { won: null }]), { number: 5, decided: false }),
    ).toEqual([{ kind: 'fight', round: 2 }])
  })

  it('does not fight twice when the first game row arrives', () => {
    expect(versusAnimations(inGame([]), inGame([{ won: null }]), { number: 5, decided: false })).toEqual([])
  })

  it('knocks out the loser, then rounds into sideboarding', () => {
    expect(versusAnimations(inGame([{ won: null }]), sideboarding([{ won: true }]), { number: 5, decided: false })).toEqual([
      { kind: 'ko', loser: 'opponent' },
      { kind: 'round', round: 2 },
    ])
    expect(versusAnimations(inGame([{ won: null }]), sideboarding([{ won: false }]), { number: 5, decided: false })).toEqual([
      { kind: 'ko', loser: 'you' },
      { kind: 'round', round: 2 },
    ])
  })

  it('calls a draw when a game ends with no winner', () => {
    expect(
      versusAnimations(inGame([{ won: null }]), inGame([{ won: null }, { won: null }]), { number: 5, decided: false }),
    ).toEqual([{ kind: 'draw' }, { kind: 'fight', round: 2 }])
  })

  it('calls a draw when the next game starts after sideboarding', () => {
    expect(
      versusAnimations(sideboarding([{ won: null }]), inGame([{ won: null }, { won: null }]), { number: 5, decided: false }),
    ).toEqual([{ kind: 'draw' }, { kind: 'fight', round: 2 }])
  })

  it('does not call a draw when sideboarding arrives before the result', () => {
    const lastMatch = { number: 5, decided: false }
    const playing = inGame([{ won: true }, { won: null }])
    const sideboard = sideboarding([{ won: true }, { won: null }])
    expect(versusAnimations(playing, sideboard, lastMatch)).toEqual([{ kind: 'round', round: 3 }])
    const late = sideboarding([{ won: true }, { won: false }])
    expect(versusAnimations(sideboard, late, lastMatch)).toEqual([{ kind: 'ko', loser: 'you' }])
  })

  it('knocks out once when the match ends in the same push', () => {
    const before = inGame([{ won: true }, { won: false }, { won: null }])
    const after = state('waiting', { record: { wins: 4, losses: 1 }, progress: { played: 5, of: 5 }, match: null })
    expect(versusAnimations(before, after, { number: 5, decided: false })).toEqual([
      { kind: 'ko', loser: 'opponent', final: true },
    ])
  })

  it('knocks out a tournament round when its record moves after the match left (no progress)', () => {
    const block = {
      series: 'challenge' as const,
      round: 4,
      stage: 'swiss' as const,
      swissRounds: 6,
      cutSize: 8,
      rank: 12,
      players: 64,
      matchesLeft: null,
      nextRoundAtMs: null,
      startsAtMs: null,
      place: null,
    }
    const round = (status: LeagueStatus, change: Partial<LeagueOverlayState>) =>
      state(status, { progress: null, tournament: block, ...change })
    const lastMatch = { number: 4, decided: false }
    const live = round('in_game', {
      record: { wins: 3, losses: 0 },
      match: { number: 4, games: [{ won: true }, { won: false }, { won: null }], gamesWon: 1, gamesLost: 1, life: null },
    })
    const left = round('waiting_for_round', { record: { wins: 3, losses: 0 }, match: null })
    expect(versusAnimations(live, left, lastMatch)).toEqual([])
    const moved = round('waiting_for_round', { record: { wins: 3, losses: 1 }, match: null })
    expect(versusAnimations(left, moved, lastMatch)).toEqual([{ kind: 'ko', loser: 'you', final: true }])
    const after = nextLastMatch(moved, lastMatch, versusAnimations(left, moved, lastMatch))
    // A bye in the next round moves the record with no match: no knock out.
    const bye = round('bye', { record: { wins: 4, losses: 1 }, match: null, tournament: { ...block, round: 5 } })
    expect(versusAnimations(moved, bye, after)).toEqual([])
  })

  it('does not knock out twice when the result was already seen', () => {
    const seen = inGame([{ won: true }, { won: false }, { won: true }])
    const after = state('waiting', { record: { wins: 4, losses: 1 }, progress: { played: 5, of: 5 }, match: null })
    expect(versusAnimations(seen, after, { number: 5, decided: true })).toEqual([])
  })

  it('knocks out once when the record moves after the match left', () => {
    const lastMatch = { number: 5, decided: false }
    const live = inGame([{ won: true }, { won: false }, { won: null }])
    const left = state('waiting', { match: null })
    expect(versusAnimations(live, left, lastMatch)).toEqual([])
    const moved = state('waiting', { match: null, record: { wins: 4, losses: 1 }, progress: { played: 5, of: 5 } })
    expect(versusAnimations(left, moved, lastMatch)).toEqual([{ kind: 'ko', loser: 'opponent', final: true }])
    const after = nextLastMatch(moved, lastMatch, versusAnimations(left, moved, lastMatch))
    expect(after).toEqual({ number: 5, decided: true })
    const again = state('waiting', { match: null, record: { wins: 5, losses: 1 }, progress: { played: 6, of: 5 } })
    expect(versusAnimations(moved, again, after)).toEqual([])
  })

  it('calls a final draw when a match ends on a drawn deciding game', () => {
    const lastMatch = { number: 5, decided: false }
    const live = inGame([{ won: true }, { won: false }, { won: null }])
    const ended = state('waiting', { match: null, record: { wins: 3, losses: 1 }, progress: { played: 5, of: 5 } })
    const played = versusAnimations(live, ended, lastMatch)
    expect(played).toEqual([{ kind: 'draw', final: true }])
    expect(nextLastMatch(ended, lastMatch, played)).toEqual({ number: 5, decided: true })
  })

  it('calls a final draw when the drawn match reaches the record after leaving', () => {
    const lastMatch = { number: 5, decided: false }
    const left = state('waiting', { match: null })
    const ended = state('waiting', { match: null, progress: { played: 5, of: 5 } })
    expect(versusAnimations(left, ended, lastMatch)).toEqual([{ kind: 'draw', final: true }])
  })

  it('does not knock out for a record move that is not the remembered match', () => {
    // Match 5 was abandoned undecided; an older match finishing later moves the record.
    const lastMatch = { number: 5, decided: false }
    const left = state('waiting', { match: null, record: { wins: 2, losses: 1 }, progress: { played: 3, of: 5 } })
    const older = state('waiting', { match: null, record: { wins: 3, losses: 1 }, progress: { played: 4, of: 5 } })
    expect(versusAnimations(left, older, lastMatch)).toEqual([])
  })
})

describe('nextLastMatch', () => {
  it('returns the live match number and whether it is decided', () => {
    expect(nextLastMatch(inGame([{ won: true }, { won: null }]), null, [])).toEqual({ number: 5, decided: false })
    expect(nextLastMatch(inGame([{ won: true }, { won: true }]), null, [])).toEqual({ number: 5, decided: true })
  })

  it('keeps the input without a live match and without a final knock out', () => {
    const lastMatch = { number: 5, decided: false }
    expect(nextLastMatch(state('waiting', { match: null }), lastMatch, [])).toBe(lastMatch)
    expect(nextLastMatch(state('waiting', { match: null }), null, [])).toBeNull()
  })
})

function versusRow(number: number, display: Partial<LeagueDisplay> = {}, size: OverlaySize = 'full'): LeagueOverlayState {
  return sampleEventState(TOURNAMENT_ROWS[number], { art: null, size, theme: 'versus', now: NOW, display: { ...DEFAULT_DISPLAY, ...display } })
}

function lineText(line: RecordLine | null): string | null {
  return line ? [line.lead, line.record, line.tail].filter((part) => part).join(' · ') : null
}

describe('the tournament rows, Versus (spec 5.4)', () => {
  // Callout rows: [title, subtitle, tone, pulse, compact title]
  const CALLOUTS: Record<number, [string, string, Callout['tone'], boolean]> = {
    1: ['GET READY', 'MODERN CHALLENGE · STARTS IN ~12 MIN', 'white', true],
    2: ['GET READY', 'MODERN CHALLENGE · STARTING SOON', 'white', true],
    5: ['ROUND 5', 'MONO GREEN TRON · 4-0 · 12 MATCHES LEFT', 'cyan', true],
    6: ['ROUND 5', 'MONO GREEN TRON · 4-0 · PAIRINGS SOON', 'cyan', true],
    7: ['ROUND 5', 'MONO GREEN TRON · 4-0 · PAIRINGS IN ~2 MIN', 'cyan', true],
    8: ['BYE', 'ROUND 3 · FREE WIN · 3-0', 'cyan', false],
    9: ['TOP 8', 'PLAYOFFS IN ~2 MIN · 3RD AFTER SWISS · 5-1', 'gold', false],
    12: ['SEMIFINAL', 'MONO GREEN TRON · 6-1 · 1 MATCH LEFT', 'cyan', true],
    13: ['CHAMPION', 'MODERN CHALLENGE · 1ST OF 64 · 8-1', 'gold', false],
    14: ['TOP 8', 'FINISHED 6TH OF 64 · 5-2', 'red', false],
    15: ['CONTINUE?', 'MISSED THE CUT · 21ST OF 64 · 4-2', 'grey', false],
    16: ['FINISHED', 'MODERN PRELIM · 12TH OF 40 · 3-1', 'white', false],
    17: ['GAME OVER', 'DROPPED AFTER ROUND 3 · 1-2', 'grey', false],
    18: ['GAME OVER', 'OUT AFTER ROUND 2 · 1-1', 'grey', false],
    19: ['INSERT COIN', 'JOIN AN EVENT ON MTGO', 'white', true],
  }

  it('calls out every row between and after matches, the same title on both sizes', () => {
    for (const [number, [title, subtitle, tone, pulse]] of Object.entries(CALLOUTS)) {
      for (const size of ['full', 'compact'] as const) {
        const shown = callout(versusRow(Number(number), { standing: true }, size), NOW)
        expect(shown, `row ${number} ${size}`).toMatchObject({ title, subtitle, tone, pulse })
      }
    }
  })

  it('steps a subtitle down by dropping parts from the end', () => {
    expect(subtitleLadder(callout(versusRow(5), NOW)!)).toEqual([
      'MONO GREEN TRON · 4-0 · 12 MATCHES LEFT',
      'MONO GREEN TRON · 4-0',
      'MONO GREEN TRON',
    ])
  })

  it('keeps the streamer label in the waiting subtitles, and leaves the deck out with Archetype off', () => {
    expect(callout(versusRow(5), NOW, 'My Brew')?.subtitle).toBe('MY BREW · 4-0 · 12 MATCHES LEFT')
    expect(callout(versusRow(5, { title: false }), NOW)?.subtitle).toBe('4-0 · 12 MATCHES LEFT')
  })

  it('calls out what comes after the last Swiss round', () => {
    const five = versusRow(5)
    const title = (change: Partial<NonNullable<LeagueOverlayState['tournament']>>) =>
      callout({ ...five, tournament: { ...five.tournament!, round: 6, ...change } }, NOW)?.title
    expect(title({})).toBe('TOP 8 NEXT')
    expect(title({ round: 4, swissRounds: 4, cutSize: null })).toBe('RESULTS SOON')
    expect(title({ swissRounds: null })).toBe('ROUND 7')
  })

  it('names the cut and the finish by their size', () => {
    const nine = versusRow(9)
    expect(callout({ ...nine, tournament: { ...nine.tournament!, cutSize: 4 } }, NOW)?.title).toBe('TOP 4')
    const fourteen = versusRow(14)
    expect(callout({ ...fourteen, tournament: { ...fourteen.tournament!, stage: 'final' } }, NOW)?.title).toBe('FINALIST')
    expect(callout({ ...fourteen, tournament: { ...fourteen.tournament!, place: null } }, NOW)?.subtitle).toBe('5-2')
  })

  it('shows the round under VS, the meta line with the rank, and the compact middle in a Swiss game', () => {
    const three = versusRow(3, { standing: true })
    expect(callout(three, NOW)).toBeNull()
    expect(matchText(three, false)).toBe('ROUND 4/6')
    expect(matchGold(three)).toBe(false)
    expect(lineText(bottomLine(three))).toBe('MODERN CHALLENGE · 3-0 · 12TH OF 64')
    expect(lineText(bottomLine(versusRow(3)))).toBe('MODERN CHALLENGE · 3-0')
    expect(compactMiddle(versusRow(3, {}, 'compact'))).toEqual({ lead: 'R4/6', record: '3-0' })
  })

  it('shows the stage in gold in a playoff game, never the rank', () => {
    const ten = versusRow(10, { standing: true })
    expect(matchText(ten, false)).toBe('SEMIFINAL')
    expect(matchGold(ten)).toBe(true)
    expect(lineText(bottomLine(ten))).toBe('MODERN CHALLENGE · 6-1')
    const at = (stage: 'top8' | 'final', wins: number) => ({
      ...versusRow(10, {}, 'compact'),
      record: { wins, losses: 1 },
      tournament: { ...ten.tournament!, stage },
    })
    expect(compactMiddle(versusRow(10, {}, 'compact'))).toEqual({ lead: 'SF', record: '6-1' })
    expect(compactMiddle(at('top8', 5))).toEqual({ lead: 'TOP 8', record: '5-1' })
    expect(compactMiddle(at('final', 7))).toEqual({ lead: 'FINAL', record: '7-1' })
  })

  it('names the next game from the match, whatever the Status switch', () => {
    expect(nextGameNumber(versusRow(4))).toBe(2)
    expect(nextGameNumber(versusRow(11))).toBe(3)
    expect(compactMiddle(versusRow(11, { status: false }, 'compact'))).toEqual({ lead: 'SF', record: '6-1' })
  })

  it('reads a payload whose tournament is null as a league card', () => {
    const nullBlock = { ...state('in_game'), tournament: null } as unknown as LeagueOverlayState
    expect(matchGold(nullBlock)).toBe(false)
    expect(matchText(nullBlock, false)).toBe('MATCH 5/5')
  })

  it('steps the meta line down: the rank first, then the event label to its kind, then the label', () => {
    expect(bottomLines(versusRow(3, { standing: true })).map(lineText)).toEqual([
      'MODERN CHALLENGE · 3-0 · 12TH OF 64',
      'MODERN CHALLENGE · 3-0',
      'CHALLENGE · 3-0',
      '3-0',
    ])
    expect(bottomLines(versusRow(10)).map(lineText)).toEqual(['MODERN CHALLENGE · 6-1', 'CHALLENGE · 6-1', '6-1'])
    expect(bottomLines(state('in_game')).map(lineText)).toEqual(['MODERN · 3-1', '3-1'])
    expect(bottomLines(versusRow(3, { record: false })).map(lineText)).toEqual(['MODERN CHALLENGE', 'CHALLENGE'])
    expect(bottomLines(versusRow(3, { event: false, format: false, record: false }))).toEqual([])
  })

  it('says GAME n while sideboarding, in tournaments as in leagues', () => {
    expect(sideboardBanner(versusRow(4))).toBe('GAME 2 · SIDEBOARDING')
    expect(sideboardBanner(versusRow(11))).toBe('GAME 3 · SIDEBOARDING')
    expect(compactMiddle(versusRow(11, {}, 'compact'))).toEqual({ lead: 'GAME 3', record: null })
  })

  it('introduces each game as GAME n, a playoff game 1 by its stage', () => {
    expect(introText(versusRow(3), 1)).toBe('GAME 1')
    expect(introText(versusRow(3), 2)).toBe('GAME 2')
    expect(introText(versusRow(10), 1)).toBe('SEMIFINAL')
    expect(introText(versusRow(10), 2)).toBe('GAME 2')
    expect(introText(state('in_game'), 1)).toBe('GAME 1')
  })
})
