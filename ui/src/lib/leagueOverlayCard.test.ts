import { describe, expect, it } from 'vitest'
import type { OverlaySize } from '@/bindings'
import type { LeagueDisplay, LeagueOverlayState, LeagueStatus } from '@/bindings'
import { sampleEventState, sampleOverlayState, TOURNAMENT_ROWS } from '@/components/overlay/overlaySamples'
import {
  cardChip,
  cardRows,
  compactLeads,
  compactSubline,
  compactTitle,
  DEFAULT_DISPLAY,
  deckTitle,
  displayOf,
  eventLabelCandidates,
  eventText,
  fullSubline,
  tournamentCard,
  labelDeckId,
  PIP_LETTERS,
  pips,
  pipTone,
  recordText,
  showsGames,
  showsMark,
} from './leagueOverlayCard'

const NOW = 1_800_000_000_000
const STATUSES: LeagueStatus[] = ['idle', 'waiting', 'in_game', 'sideboarding', 'complete', 'trophied', 'dropped']
const NOTHING: LeagueDisplay = {
  games: 'dots',
  event: false,
  format: false,
  title: false,
  progress: false,
  status: false,
  record: false,
  standing: false,
}

function sample(status: LeagueStatus, size: OverlaySize = 'full', display: Partial<LeagueDisplay> = {}): LeagueOverlayState {
  return sampleOverlayState(status, { art: null, size, display: { ...DEFAULT_DISPLAY, ...display } })
}

function withEvent(name: string, format: string, display: Partial<LeagueDisplay> = {}): LeagueOverlayState {
  return { ...sample('in_game', 'full', display), event: { kind: 'league', name, format } }
}

describe('the display choices', () => {
  it('default to everything on with dots', () => {
    expect(DEFAULT_DISPLAY).toEqual({
      games: 'dots',
      event: true,
      format: true,
      title: true,
      progress: true,
      status: true,
      record: true,
      standing: false,
    })
  })

  it('fill a payload without them, or with some missing, from the defaults', () => {
    const bare = { ...sample('in_game') } as Partial<LeagueOverlayState>
    delete bare.display
    expect(displayOf(bare as LeagueOverlayState)).toEqual(DEFAULT_DISPLAY)
    const partial = { ...sample('in_game'), display: { record: false } as LeagueDisplay }
    expect(displayOf(partial)).toEqual({ ...DEFAULT_DISPLAY, record: false })
  })
})

describe('the league card chip', () => {
  it('names every status as 0.x does', () => {
    expect(STATUSES.map((status) => cardChip(sample(status), NOW))).toEqual([
      null,
      { tone: 'neutral', label: 'Next: match 5' },
      { tone: 'neutral', label: 'Match 5/5' },
      { tone: 'amber', label: 'Sideboarding', icon: 'swap' },
      { tone: 'green', label: 'Complete' },
      { tone: 'trophy', label: 'Trophied!', icon: 'trophy' },
      { tone: 'red', label: 'Dropped' },
    ])
  })

  it('falls back to match 1 of 5 and next match 1 without numbers', () => {
    const bare = { ...sample('in_game'), match: null, progress: null }
    expect(cardChip(bare, NOW)).toEqual({ tone: 'neutral', label: 'Match 1/5' })
    expect(cardChip({ ...bare, status: 'waiting' }, NOW)).toEqual({ tone: 'neutral', label: 'Next: match 1' })
  })

  it('leaves out the match progress and the state chips when they are off', () => {
    expect(cardChip(sample('in_game', 'full', { progress: false }), NOW)).toBeNull()
    expect(cardChip(sample('waiting', 'full', { progress: false }), NOW)).toBeNull()
    expect(cardChip(sample('in_game', 'full', { status: false }), NOW)).toEqual({ tone: 'neutral', label: 'Match 5/5' })
    for (const status of ['sideboarding', 'complete', 'trophied', 'dropped'] as const) {
      expect(cardChip(sample(status, 'full', { status: false }), NOW)).toBeNull()
    }
  })
})

describe('the event chip', () => {
  it('shows a league as its format, by the switches (spec 5.2)', () => {
    expect(eventText(withEvent('FRAX6 League', 'Sealed'))).toBe('Sealed')
    expect(eventText(withEvent('Competitive League', 'Modern'))).toBe('Modern')
    expect(eventText(withEvent('FRAX6 League', 'Sealed', { format: false }))).toBe('League')
    expect(eventText(withEvent('FRAX6 League', 'Sealed', { event: false }))).toBe('Sealed')
    expect(eventText(withEvent('FRAX6 League', 'Sealed', { event: false, format: false }))).toBeNull()
  })

  it('names the kind alone for an empty format', () => {
    expect(eventText(withEvent('FRAX6 League', ''))).toBe('League')
    expect(eventText(withEvent('FRAX6 League', '  ', { event: false }))).toBeNull()
  })

  it('is empty without an event', () => {
    expect(eventText(sample('idle'))).toBeNull()
  })
})

describe('the league card lines', () => {
  it('reads the state on the full card, with no game number', () => {
    expect(STATUSES.map((status) => fullSubline(sample(status), NOW))).toEqual([
      'Join a league or Challenge on MTGO',
      'Looking for an opponent…',
      '',
      'Game 2 next',
      '9-4 in games',
      '10-2 in games',
      'Dropped after match 3',
    ])
  })

  it('reads the state on the compact card, with no game number', () => {
    expect(STATUSES.map((status) => compactSubline(sample(status, 'compact'), NOW))).toEqual([
      'Join a league or Challenge on MTGO',
      'Next: match 5',
      '',
      'Game 2 next',
      'Complete',
      'Trophied!',
      'Dropped',
    ])
  })

  it('says looking for an opponent on the compact card when match progress is off', () => {
    expect(compactSubline(sample('waiting', 'compact', { progress: false }), NOW)).toBe('Looking for an opponent…')
    expect(compactSubline(sample('waiting', 'compact', { progress: false, status: false }), NOW)).toBe('')
  })

  it('drops the state text when status is off', () => {
    for (const status of STATUSES) {
      expect(fullSubline(sample(status, 'full', { status: false }), NOW)).toBe('')
    }
    expect(compactSubline(sample('sideboarding', 'compact', { status: false }), NOW)).toBe('')
    expect(compactSubline(sample('complete', 'compact', { status: false }), NOW)).toBe('')
  })

  it('shows no game record when a finished run has none', () => {
    expect(fullSubline({ ...sample('complete'), gameRecord: null }, NOW)).toBe('')
  })

  it('shows the games while a match is on, whatever the switches', () => {
    expect(STATUSES.filter((status) => showsGames(sample(status, 'full', NOTHING)))).toEqual(['in_game', 'sideboarding'])
  })

  it('shows the mymtgo.com mark only when idle with status on', () => {
    expect(showsMark(sample('idle'))).toBe(true)
    expect(showsMark(sample('idle', 'full', { status: false }))).toBe(false)
    expect(showsMark(sample('waiting'))).toBe(false)
  })
})

describe('the full card rows', () => {
  it('keeps both rows by default', () => {
    for (const status of STATUSES) {
      expect(cardRows(sample(status), NOW), status).toEqual({ top: status !== 'idle', bottom: true })
    }
  })

  it('collapses the top row when its chips are off', () => {
    expect(cardRows(sample('in_game', 'full', { event: false, format: false, progress: false }), NOW)).toEqual({
      top: false,
      bottom: true,
    })
  })

  it('keeps games and record on one row for the bare layout', () => {
    const bare = sample('in_game', 'full', { ...NOTHING, record: true })
    expect(cardRows(bare, NOW)).toEqual({ top: false, bottom: true })
    expect(recordText(bare)).toBe('3-1')
    expect(showsGames(bare)).toBe(true)
  })

  it('hides everything with every switch off', () => {
    const waiting = sample('waiting', 'full', NOTHING)
    expect(cardRows(waiting, NOW)).toEqual({ top: false, bottom: false })
    expect(recordText(waiting)).toBeNull()
    expect(cardRows(sample('idle', 'full', NOTHING), NOW)).toEqual({ top: false, bottom: false })
  })
})

describe('the league card title', () => {
  const deck = sample('in_game').deck!

  it('prefers the label, then the archetype, the deck name and "Unknown deck"', () => {
    const state = sample('in_game')
    expect(compactTitle(state, 'Qstream deck')).toBe('Qstream deck')
    expect(compactTitle(state, '')).toBe('Mono Green Tron')
    expect(deckTitle({ ...state, deck: { ...deck, archetype: null } })).toBe('tron v3 final')
    expect(deckTitle({ ...state, deck: null })).toBe('Unknown deck')
  })

  it('reads "Not in an event" when idle, whatever the deck', () => {
    const idle = { ...sample('idle'), deck }
    expect(deckTitle(idle)).toBe('Not in an event')
    expect(compactTitle(idle, '')).toBe('Not in an event')
  })

  it('edits the label only on a run with a deck', () => {
    expect(labelDeckId(sample('in_game'))).toBe(0)
    expect(labelDeckId({ ...sample('idle'), deck })).toBeNull()
    expect(labelDeckId({ ...sample('waiting'), deck: null })).toBeNull()
  })

  it('shows the run record, none when idle or switched off', () => {
    expect(recordText(sample('in_game'))).toBe('3-1')
    expect(recordText(sample('idle'))).toBeNull()
    expect(recordText(sample('in_game', 'full', { record: false }))).toBeNull()
  })
})

describe('the game pips', () => {
  it('pads to best of three', () => {
    expect(pips([{ won: true }], false)).toEqual([
      { key: 0, won: true, live: false, played: true },
      { key: 1, won: null, live: false, played: false },
      { key: 2, won: null, live: false, played: false },
    ])
  })

  it('goes past three when a draw pushed the match further', () => {
    const games = [{ won: true }, { won: null }, { won: false }, { won: null }]
    expect(pips(games, false).map((pip) => pip.won)).toEqual([true, null, false, null])
  })

  it('marks the last game live while its result is unknown', () => {
    expect(pips([{ won: true }, { won: false }, { won: null }], true).map((pip) => pip.live)).toEqual([false, false, true])
    expect(pips([{ won: true }, { won: false }], true).map((pip) => pip.live)).toEqual([false, false, false])
  })

  it('makes game 1 live before its first game row', () => {
    expect(pips([], true).map((pip) => pip.live)).toEqual([true, false, false])
    expect(pips([], false).map((pip) => pip.live)).toEqual([false, false, false])
  })

  it('reads each game as a letter', () => {
    const letters = (games: Array<{ won: boolean | null }>, live: boolean) =>
      pips(games, live).map((pip) => PIP_LETTERS[pipTone(pip)])
    expect(letters([{ won: true }, { won: false }, { won: null }], true)).toEqual(['W', 'L', '-'])
    expect(letters([], true)).toEqual(['-', '-', '-'])
    expect(letters([{ won: true }, { won: null }], false)).toEqual(['W', 'D', '-'])
    expect(pips([{ won: true }, { won: null }], true).map(pipTone)).toEqual(['won', 'live', 'unplayed'])
  })
})

describe('the preview samples', () => {
  it('swap in the real deck and keep the look', () => {
    const real = { id: 7, name: 'Qreal deck', archetype: null, colorIdentity: ['R'], label: 'Qlabel' }
    const state = sampleOverlayState('waiting', { art: 'https://cards.example/q.jpg', size: 'compact', deck: real })
    expect(state.deck).toEqual(real)
    expect(state.art).toEqual({ url: 'https://cards.example/q.jpg', thumbnail: false })
    expect(state.size).toBe('compact')
    expect(state.display).toEqual(DEFAULT_DISPLAY)
    const idle = sampleOverlayState('idle', { art: null, size: 'full' })
    expect([idle.event, idle.deck, idle.art]).toEqual([null, null, null])
  })

  it('carry the chosen display and the thumbnail', () => {
    const state = sampleOverlayState('in_game', {
      art: 'https://cards.example/q.jpg',
      size: 'full',
      display: NOTHING,
      thumbnail: true,
    })
    expect(state.display).toEqual(NOTHING)
    expect(state.art).toEqual({ url: 'https://cards.example/q.jpg', thumbnail: true })
  })
})

function row(number: number, display: Partial<LeagueDisplay> = {}, size: OverlaySize = 'full'): LeagueOverlayState {
  return sampleEventState(TOURNAMENT_ROWS[number], { art: null, size, now: NOW, display: { ...DEFAULT_DISPLAY, ...display } })
}

describe('the tournament rows, Classic Full (spec 5.4)', () => {
  const STANDING = { standing: true }
  const DECK = 'Mono Green Tron'
  // Row: [title, title icon, chip [tone, label, icon] or null, status line parts, record]
  type Row = [string | null, string | null, [string, string, string | null] | null, string[], string | null]
  const FULL: Record<number, Row> = {
    1: ['Starts in ~12 min', null, ['neutral', 'Registered', null], [DECK], null],
    2: ['Starting soon', null, ['neutral', 'Registered', null], [DECK], null],
    3: ['Round 4/6', null, ['neutral', '12th of 64', null], [DECK], '3-0'],
    4: ['Round 4/6', null, ['amber', 'Sideboarding', 'swap'], ['Game 2 next', DECK], '3-0'],
    5: ['Round 5 next', null, ['neutral', '12 matches left', null], [DECK, '12th of 64'], '4-0'],
    6: ['Round 5 next', null, ['neutral', 'Pairings soon', null], [DECK, '12th of 64'], '4-0'],
    7: ['Round 5 next', null, ['neutral', 'Pairings ~2 min', null], [DECK, '12th of 64'], '4-0'],
    8: ['Round 3/6', null, ['neutral', 'Bye', null], ['Free win', DECK], '3-0'],
    9: ['Top 8!', null, ['sky', 'Playoffs ~2 min', null], [DECK, '3rd after Swiss'], '5-1'],
    10: ['Semifinal', null, null, [DECK], '6-1'],
    11: ['Semifinal', null, ['amber', 'Sideboarding', 'swap'], ['Game 3 next', DECK], '6-1'],
    12: ['Semifinal next', null, ['sky', '1 match left', null], [DECK], '6-1'],
    13: ['Champion', 'trophy', ['trophy', '1st of 64', null], [DECK, '17-5 in games'], '8-1'],
    14: ['Top 8', null, ['green', '6th of 64', null], [DECK], '5-2'],
    15: ['Missed cut', null, ['neutral', '21st of 64', null], [DECK], '4-2'],
    16: ['Finished', null, ['green', '12th of 40', null], [DECK], '3-1'],
    17: ['Dropped', null, ['red', 'After round 3', null], [DECK], '1-2'],
    18: ['Out', null, ['neutral', 'After round 2', null], [DECK], '1-1'],
  }

  function read(state: LeagueOverlayState, label = '') {
    const card = tournamentCard(state, NOW, label)!
    const chip = card.chip === null ? null : [card.chip.tone, card.chip.label, card.chip.icon ?? null]
    return [card.title?.text ?? null, card.title?.icon ?? null, chip, card.line]
  }

  it('reads every row with Standing on', () => {
    for (const [number, [title, icon, chip, line, record]] of Object.entries(FULL)) {
      const state = row(Number(number), STANDING)
      expect(read(state), `row ${number}`).toEqual([title, icon, chip, line])
      expect(recordText(state), `row ${number}`).toBe(record)
      expect(cardChip(state, NOW), `row ${number}`).toEqual(tournamentCard(state, NOW)!.chip)
    }
  })

  it('shows the rank only with Standing on, in the chip while live and in the line between rounds', () => {
    expect(tournamentCard(row(3), NOW)!.chip).toBeNull()
    expect(tournamentCard(row(5), NOW)!.line).toEqual([DECK])
    expect(tournamentCard(row(7), NOW)!.line).toEqual([DECK])
    expect(tournamentCard({ ...row(10, STANDING) }, NOW)!.chip).toBeNull()
  })

  it('falls back to the archetype title with the title switch off, never showing the archetype twice', () => {
    const fiveNoProgress = tournamentCard(row(5, { progress: false, standing: true }), NOW)!
    expect([fiveNoProgress.title, fiveNoProgress.chip?.label, fiveNoProgress.line]).toEqual([null, '12 matches left', ['12th of 64']])
    const threeNoProgress = tournamentCard(row(3, { progress: false }), NOW)!
    expect([threeNoProgress.title, threeNoProgress.line]).toEqual([null, []])
    const wonNoStatus = tournamentCard(row(13, { status: false }), NOW)!
    expect([wonNoStatus.title, wonNoStatus.chip, wonNoStatus.line]).toEqual([null, null, []])
    const registeredNoStatus = tournamentCard(row(1, { status: false }), NOW)!
    expect([registeredNoStatus.title, registeredNoStatus.chip, registeredNoStatus.line]).toEqual([null, null, []])
  })

  it('keeps the round title with Status off, dropping the status details', () => {
    const four = tournamentCard(row(4, { status: false }), NOW)!
    expect([four.title?.text, four.chip, four.line]).toEqual(['Round 4/6', null, [DECK]])
    const five = tournamentCard(row(5, { status: false, standing: true }), NOW)!
    expect([five.title?.text, five.chip, five.line]).toEqual(['Round 5 next', null, [DECK, '12th of 64']])
  })

  it('leaves the archetype out of the line with Archetype off', () => {
    const three = tournamentCard(row(3, { title: false }), NOW)!
    expect([three.title?.text, three.line]).toEqual(['Round 4/6', []])
    expect(tournamentCard(row(13, { title: false }), NOW)!.line).toEqual(['17-5 in games'])
  })

  it('names the deck by the streamer label in the line', () => {
    expect(tournamentCard(row(3), NOW, 'My Brew')!.line).toEqual(['My Brew'])
  })

  it('names the playoff stage, with a trophy before Final, and the finish by the stage lost', () => {
    const ten = row(10)
    const title = (stage: 'top8' | 'final') => tournamentCard({ ...ten, tournament: { ...ten.tournament!, stage } }, NOW)!.title
    expect(title('top8')).toEqual({ text: 'Top 8' })
    expect(title('final')).toEqual({ text: 'Final', icon: 'trophy' })
    const fourteen = row(14)
    const lost = (stage: 'semifinal' | 'final') => tournamentCard({ ...fourteen, tournament: { ...fourteen.tournament!, stage } }, NOW)!.title?.text
    expect([lost('semifinal'), lost('final')]).toEqual(['Top 4', 'Finalist'])
  })

  it('leaves unknown values out', () => {
    const three = row(3)
    expect(tournamentCard({ ...three, tournament: { ...three.tournament!, swissRounds: null } }, NOW)!.title?.text).toBe('Round 4')
    const fourteen = row(14)
    expect(tournamentCard({ ...fourteen, tournament: { ...fourteen.tournament!, place: null } }, NOW)!.chip).toBeNull()
  })

  it('names what comes after the last Swiss round: the cut, or the results without one', () => {
    const five = row(5)
    const after = (change: Partial<NonNullable<LeagueOverlayState['tournament']>>, size: OverlaySize = 'full') =>
      tournamentCard({ ...row(5, {}, size), tournament: { ...five.tournament!, round: 6, ...change } }, NOW)!.title?.text
    expect(after({})).toBe('Top 8 next')
    expect(after({}, 'compact')).toBe('Top 8 next')
    expect(after({ cutSize: 4 })).toBe('Top 4 next')
    expect(after({ round: 4, swissRounds: 4, cutSize: null })).toBe('Results soon')
    expect(after({ swissRounds: null })).toBe('Round 7 next')
    expect(after({ round: 5 })).toBe('Round 6 next')
  })

  it('shows draws in the record', () => {
    expect(recordText({ ...row(5), record: { wins: 3, losses: 0, draws: 1 } })).toBe('3-0-1')
  })

  it('puts format plus kind in the left chip', () => {
    expect(eventText(row(3))).toBe('Modern Challenge')
    expect(eventLabelCandidates(row(3))).toEqual(['Modern Challenge', 'Challenge'])
    expect(eventLabelCandidates(sample('in_game'))).toEqual(['Modern'])
    expect(eventLabelCandidates(sample('idle'))).toEqual([])
    expect(eventText(row(16))).toBe('Modern Prelim')
    expect(eventText(row(3, { event: false }))).toBe('Modern')
    expect(eventText(row(3, { format: false }))).toBe('Challenge')
    expect(eventText(row(3, { event: false, format: false }))).toBeNull()
  })

  it('says the idle text on an idle card', () => {
    expect(tournamentCard(row(19), NOW)).toBeNull()
    expect(deckTitle(row(19))).toBe('Not in an event')
    expect(fullSubline(row(19), NOW)).toBe('Join a league or Challenge on MTGO')
  })
})

describe('the tournament rows, Classic Compact (spec 5.4)', () => {
  const DECK = 'Mono Green Tron'
  // Row: [title, line parts]; no chip, so the line carries its detail; never the rank.
  const COMPACT: Record<number, [string, string[]]> = {
    1: ['Starts in ~12 min', [DECK]],
    2: ['Starting soon', [DECK]],
    3: ['Round 4/6', [DECK]],
    4: ['Round 4/6', ['Game 2 next']],
    5: ['Round 5 next', ['12 matches left']],
    6: ['Round 5 next', ['Pairings soon']],
    7: ['Round 5 next', ['Pairings in ~2 min']],
    8: ['Round 3/6', ['Bye', 'free win']],
    9: ['Top 8!', ['Playoffs in ~2 min']],
    10: ['Semifinal', [DECK]],
    11: ['Semifinal', ['Game 3 next']],
    12: ['Semifinal next', ['1 match left']],
    13: ['Champion', ['1st of 64']],
    14: ['Top 8', ['6th of 64']],
    15: ['Missed cut', ['21st of 64']],
    16: ['Finished', ['12th of 40']],
    17: ['Dropped', ['After round 3']],
    18: ['Out', ['After round 2']],
  }

  it('reads every row, never the rank, with no lead', () => {
    for (const [number, [title, line]] of Object.entries(COMPACT)) {
      const state = row(Number(number), { standing: true }, 'compact')
      const card = tournamentCard(state, NOW)!
      expect([card.title?.text, card.compactLine], `row ${number}`).toEqual([title, line])
      expect(compactLeads(state), `row ${number}`).toEqual([])
    }
  })

  it('falls back to the archetype title with the title switch off', () => {
    const card = tournamentCard(row(5, { progress: false }, 'compact'), NOW)!
    expect([card.title, card.compactLine]).toEqual([null, ['12 matches left']])
    const registered = tournamentCard(row(1, { status: false }, 'compact'), NOW)!
    expect([registered.title, registered.compactLine]).toEqual([null, []])
  })
})

describe('the league rows with the new wording', () => {
  it('uses a slash in the match chip and leads the compact line with the match', () => {
    expect(cardChip(sample('in_game'), NOW)).toEqual({ tone: 'neutral', label: 'Match 5/5' })
    expect(compactLeads(sample('in_game', 'compact'))).toEqual(['Match 5/5', '5 of 5'])
    expect(compactLeads(sample('sideboarding', 'compact'))).toEqual([])
    expect(compactLeads(sample('in_game', 'compact', { progress: false }))).toEqual([])
  })
})
