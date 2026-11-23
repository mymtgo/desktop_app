import type { LeagueDeck, LeagueTheme, OverlaySize } from '@/bindings'
import type {
  LeagueDisplay,
  LeagueOverlayState,
  LeagueStatus,
  OtherEvent,
  TournamentBlock,
} from '@/bindings'
import { DEFAULT_DISPLAY } from '@/lib/leagueOverlayCard'

/**
 * The event card's preview states (0.x `overlaySamples.ts`, event overlay
 * spec 5.5): the Overlays settings page shows each state with the real last
 * played deck so its label can be edited. League samples are named by their
 * status; tournament samples are one per row of spec 5.4 (a Modern Challenge
 * 64: 6 Swiss rounds, top 8). Times are relative to `now`, so the card's
 * countdowns render.
 */
const sampleDeck: LeagueDeck = {
  id: 0,
  name: 'tron v3 final',
  archetype: 'Mono Green Tron',
  colorIdentity: ['G'],
  label: null,
}
// A name that does not repeat its format, so the preview shows the Format switch working.
const leagueEvent = { kind: 'league' as const, name: 'Competitive League', format: 'Modern' }
const challengeEvent = { kind: 'tournament' as const, name: 'Modern Challenge 64', format: 'Modern' }
const preliminaryEvent = { kind: 'tournament' as const, name: 'Modern Preliminary', format: 'Modern' }

const MINUTE = 60_000

/** The league samples: every league status, and a league match with a Challenge in the strip. */
export type LeagueSample = Exclude<LeagueStatus, TournamentOnlyStatus> | 'league_with_challenge'

type TournamentOnlyStatus = 'registered' | 'waiting_for_round' | 'bye' | 'made_cut' | 'top_cut' | 'missed_cut' | 'eliminated'

/** The tournament samples, one per row of spec 5.4 (row 19, idle, is the league `idle`), and the strip sample. */
export type TournamentSample =
  | 'challenge_registered'
  | 'challenge_starting'
  | 'challenge_round'
  | 'challenge_round_sideboarding'
  | 'challenge_waiting'
  | 'challenge_waiting_soon'
  | 'challenge_between_rounds'
  | 'challenge_bye'
  | 'challenge_made_cut'
  | 'challenge_playoff'
  | 'challenge_playoff_sideboarding'
  | 'challenge_playoff_waiting'
  | 'challenge_won'
  | 'challenge_top_cut'
  | 'challenge_missed_cut'
  | 'preliminary_complete'
  | 'challenge_dropped'
  | 'challenge_eliminated'
  | 'challenge_with_league'

export type PreviewSample = LeagueSample | TournamentSample

/** Spec 5.4 row number to its sample (row 19, idle, is shared with leagues). */
export const TOURNAMENT_ROWS: Record<number, PreviewSample> = {
  1: 'challenge_registered',
  2: 'challenge_starting',
  3: 'challenge_round',
  4: 'challenge_round_sideboarding',
  5: 'challenge_waiting',
  6: 'challenge_waiting_soon',
  7: 'challenge_between_rounds',
  8: 'challenge_bye',
  9: 'challenge_made_cut',
  10: 'challenge_playoff',
  11: 'challenge_playoff_sideboarding',
  12: 'challenge_playoff_waiting',
  13: 'challenge_won',
  14: 'challenge_top_cut',
  15: 'challenge_missed_cut',
  16: 'preliminary_complete',
  17: 'challenge_dropped',
  18: 'challenge_eliminated',
  19: 'idle',
}

export interface SampleBase {
  art: string | null
  size: OverlaySize
  deck?: LeagueDeck | null
  display?: LeagueDisplay
  theme?: LeagueTheme
  thumbnail?: boolean
  /** The time the sample's estimates count from, UTC ms. */
  now?: number
}

function common(base: SampleBase) {
  return {
    art: base.art ? { url: base.art, thumbnail: base.thumbnail ?? false } : null,
    size: base.size,
    deck: base.deck ?? sampleDeck,
    event: leagueEvent,
    display: base.display ?? DEFAULT_DISPLAY,
    theme: base.theme ?? ('classic' as const),
    record: null,
    progress: null,
    match: null,
    gameRecord: null,
  }
}

/** A league preview state; `base.deck` swaps in the real last played deck. */
export function sampleOverlayState(status: LeagueStatus, base: SampleBase): LeagueOverlayState {
  const shared = common(base)
  switch (status) {
    case 'in_game':
      return {
        ...shared,
        status,
        record: { wins: 3, losses: 1 },
        progress: { played: 4, of: 5 },
        match: {
          number: 5,
          games: [{ won: true }, { won: false }, { won: null }],
          gamesWon: 1,
          gamesLost: 1,
          life: { you: 14, opponent: 4, startYou: 20, startOpponent: 20 },
          opponentArchetype: 'Dimir Tempo',
        },
      }
    case 'sideboarding':
      return {
        ...shared,
        status,
        record: { wins: 3, losses: 1 },
        progress: { played: 4, of: 5 },
        match: { number: 5, games: [{ won: true }], gamesWon: 1, gamesLost: 0, life: null },
      }
    case 'waiting':
      return { ...shared, status, record: { wins: 3, losses: 1 }, progress: { played: 4, of: 5 } }
    case 'complete':
      return { ...shared, status, record: { wins: 4, losses: 1 }, progress: { played: 5, of: 5 }, gameRecord: { won: 9, lost: 4 } }
    case 'trophied':
      return { ...shared, status, record: { wins: 5, losses: 0 }, progress: { played: 5, of: 5 }, gameRecord: { won: 10, lost: 2 } }
    case 'dropped':
      return { ...shared, status, record: { wins: 1, losses: 2 }, progress: { played: 3, of: 5 }, gameRecord: { won: 3, lost: 5 } }
    default:
      return { ...shared, status: 'idle', event: null, deck: null }
  }
}

/** The Swiss part of the sample Challenge; each sample changes what its row needs. */
const CHALLENGE: TournamentBlock = {
  series: 'challenge',
  round: 4,
  stage: 'swiss',
  swissRounds: 6,
  cutSize: 8,
  rank: 12,
  players: 64,
  matchesLeft: null,
  nextRoundAtMs: null,
  startsAtMs: null,
  place: null,
}

const LIFE = { you: 20, opponent: 11, startYou: 20, startOpponent: 20 }

/** The Challenge waiting for round 5, as the strip shows it beside a league match. */
function challengeInStrip(now: number): OtherEvent {
  return {
    kind: 'tournament',
    name: challengeEvent.name,
    status: 'waiting_for_round',
    record: { wins: 4, losses: 0 },
    match: null,
    tournament: { ...CHALLENGE, matchesLeft: 12, nextRoundAtMs: now + 14 * MINUTE },
  }
}

/** The league match live, as the strip shows it beside a Challenge round. */
const LEAGUE_IN_STRIP: OtherEvent = {
  kind: 'league',
  name: leagueEvent.name,
  status: 'in_game',
  record: { wins: 3, losses: 1 },
  match: { game: 2, gamesWon: 1, gamesLost: 0 },
  tournament: null,
}

function tournamentState(sample: TournamentSample, base: SampleBase, now: number): LeagueOverlayState {
  const shared = { ...common(base), event: challengeEvent }
  const block = (change: Partial<TournamentBlock>): TournamentBlock => ({ ...CHALLENGE, ...change })
  const live = (number: number, games: Array<{ won: boolean | null }>, life: typeof LIFE | null, opponentArchetype?: string) => ({
    number,
    games,
    gamesWon: games.filter((game) => game.won === true).length,
    gamesLost: games.filter((game) => game.won === false).length,
    life,
    opponentArchetype,
  })
  switch (sample) {
    case 'challenge_registered':
      return { ...shared, status: 'registered', tournament: block({ round: 1, rank: null, players: null, startsAtMs: now + 12 * MINUTE }) }
    case 'challenge_starting':
      return { ...shared, status: 'registered', tournament: block({ round: 1, rank: null, players: null, startsAtMs: now - MINUTE }) }
    case 'challenge_round':
      return { ...shared, status: 'in_game', record: { wins: 3, losses: 0 }, match: live(4, [{ won: true }, { won: null }], LIFE, 'Boros Energy'), tournament: block({}) }
    case 'challenge_with_league':
      return { ...tournamentState('challenge_round', base, now), other: LEAGUE_IN_STRIP }
    case 'challenge_round_sideboarding':
      return { ...shared, status: 'sideboarding', record: { wins: 3, losses: 0 }, match: live(4, [{ won: true }], null), tournament: block({}) }
    case 'challenge_waiting':
      return {
        ...shared,
        status: 'waiting_for_round',
        record: { wins: 4, losses: 0 },
        tournament: block({ matchesLeft: 12 }),
      }
    case 'challenge_waiting_soon':
      return {
        ...shared,
        status: 'waiting_for_round',
        record: { wins: 4, losses: 0 },
        tournament: block({ matchesLeft: 0, nextRoundAtMs: now - MINUTE }),
      }
    case 'challenge_between_rounds':
      return {
        ...shared,
        status: 'waiting_for_round',
        record: { wins: 4, losses: 0 },
        tournament: block({ matchesLeft: 0, nextRoundAtMs: now + 2 * MINUTE }),
      }
    case 'challenge_bye':
      return { ...shared, status: 'bye', record: { wins: 3, losses: 0 }, tournament: block({ round: 3 }) }
    case 'challenge_made_cut':
      return { ...shared, status: 'made_cut', record: { wins: 5, losses: 1 }, tournament: block({ round: 6, rank: 3, nextRoundAtMs: now + 2 * MINUTE }) }
    case 'challenge_playoff':
      return {
        ...shared,
        status: 'in_game',
        record: { wins: 6, losses: 1 },
        match: live(8, [{ won: false }, { won: true }, { won: null }], { you: 9, opponent: 13, startYou: 20, startOpponent: 20 }),
        tournament: block({ round: 8, stage: 'semifinal', rank: 3 }),
      }
    case 'challenge_playoff_sideboarding':
      return {
        ...shared,
        status: 'sideboarding',
        record: { wins: 6, losses: 1 },
        match: live(8, [{ won: false }, { won: true }], null),
        tournament: block({ round: 8, stage: 'semifinal', rank: 3 }),
      }
    case 'challenge_playoff_waiting':
      return {
        ...shared,
        status: 'waiting_for_round',
        record: { wins: 6, losses: 1 },
        tournament: block({ round: 7, stage: 'top8', rank: 3, matchesLeft: 1 }),
      }
    case 'challenge_won':
      return {
        ...shared,
        status: 'trophied',
        record: { wins: 8, losses: 1 },
        gameRecord: { won: 17, lost: 5 },
        tournament: block({ round: 9, stage: 'final', rank: 1, place: 1 }),
      }
    case 'challenge_top_cut':
      return {
        ...shared,
        status: 'top_cut',
        record: { wins: 5, losses: 2 },
        gameRecord: { won: 11, lost: 6 },
        tournament: block({ round: 7, stage: 'top8', rank: 6, place: 6 }),
      }
    case 'challenge_missed_cut':
      return {
        ...shared,
        status: 'missed_cut',
        record: { wins: 4, losses: 2 },
        gameRecord: { won: 9, lost: 6 },
        tournament: block({ round: 6, rank: 21, place: 21 }),
      }
    case 'preliminary_complete':
      return {
        ...shared,
        event: preliminaryEvent,
        status: 'complete',
        record: { wins: 3, losses: 1 },
        gameRecord: { won: 7, lost: 3 },
        tournament: block({ series: 'preliminary', round: 4, swissRounds: 4, cutSize: null, rank: 12, players: 40, place: 12 }),
      }
    case 'challenge_dropped':
      return { ...shared, status: 'dropped', record: { wins: 1, losses: 2 }, tournament: block({ round: 3, rank: 40 }) }
    case 'challenge_eliminated':
      return { ...shared, status: 'eliminated', record: { wins: 1, losses: 1 }, tournament: block({ round: 2, rank: 50 }) }
  }
}

function isTournamentSample(sample: PreviewSample): sample is TournamentSample {
  return sample.startsWith('challenge_') || sample.startsWith('preliminary_')
}

/** Any preview sample, league or tournament. `base.now` defaults to the current time. */
export function sampleEventState(sample: PreviewSample, base: SampleBase): LeagueOverlayState {
  const now = base.now ?? Date.now()
  if (isTournamentSample(sample)) return tournamentState(sample, base, now)
  if (sample === 'league_with_challenge') return { ...sampleOverlayState('in_game', base), other: challengeInStrip(now) }
  return sampleOverlayState(sample, base)
}
