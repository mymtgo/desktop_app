import { describe, expect, it } from 'vitest'
import type { WrappedCard } from '@/bindings'
import { cardView } from './copy'
import { TEST_BUNDLES } from './testBundles'

const BUNDLE = TEST_BUNDLES.grinder

function view(card: WrappedCard) {
  return cardView(card, BUNDLE)
}

describe('the Wrapped card words', () => {
  it('the opening card counts the year\'s sets and carries its winners sentence', () => {
    const shown = view({ kind: 'magicYear' })
    expect(shown.label).toBe('2026 in Magic')
    expect(shown.title).toBe('This year saw 7 set releases')
    const year = shown.visual?.kind === 'magicYear' ? shown.visual.year : null
    expect(year?.sets.map((set) => set.code)).toEqual(['ecl', 'tmt', 'sos', 'msh', 'hob', 'fra', 'trk'])
    expect(year?.winners.filter((part) => typeof part !== 'string')).toHaveLength(4)
  })

  it('the banned card lists each card once with every format it left', () => {
    const shown = view({ kind: 'banned' })
    expect(shown.title).toBe('A moment of silence...')
    const cards = shown.visual?.kind === 'banned' ? shown.visual.cards : []
    expect(cards.find((card) => card.name === 'The Fantasticar')?.formats).toEqual(['Legacy', 'Vintage (restricted)'])
    expect(cards.find((card) => card.name === 'Lotus Field')).toEqual({ name: 'Lotus Field', formats: ['Modern'], date: '2026-05-18' })
  })

  it('the numbers card shows matches, games, hours and days as figures under the activity grid', () => {
    const shown = view({ kind: 'yearInNumbers', matches: 1, games: 2, hours: 1, daysPlayed: 1, byDay: [1, 0], topPercent: null })
    expect(shown.visual).toEqual({ kind: 'days', byDay: [1, 0] })
    expect(shown.figures.map((figure) => `${figure.value} ${figure.label}`)).toEqual(['1 match', '2 games', '1 hour', '1 day'])
  })

  it('the numbers card leaves hours out when there are none', () => {
    const shown = view({ kind: 'yearInNumbers', matches: 30, games: 60, hours: 0, daysPlayed: 5, byDay: [], topPercent: null })
    expect(shown.figures.map((figure) => figure.label)).toEqual(['matches', 'games', 'days'])
  })

  it('the top percent badge shows only for the top half', () => {
    const card = (topPercent: number | null): WrappedCard => ({ kind: 'yearInNumbers', matches: 296, games: 702, hours: 118, daysPlayed: 71, byDay: [], topPercent })
    expect(view(card(3)).badge).toBe("You're in the top 3% of MyMTGO players")
    expect(view(card(50)).badge).toBe("You're in the top 50% of MyMTGO players")
    expect(view(card(51)).badge).toBeNull()
    expect(view(card(null)).badge).toBeNull()
  })

  it('the formats card leads with the favourite and its art', () => {
    const art = { remote: 'https://example.test/art.jpg', local: null, custom: null }
    const shown = view({ kind: 'formats', favourite: { label: 'Modern', matches: 899, percent: 70, art }, mentions: [{ label: 'Pauper', matches: 231, percent: 18 }] })
    expect(shown.label).toBe('Your format of choice')
    expect(shown.title).toBe('Modern')
    expect(shown.lines).toEqual(['899 matches, 70% of your year.'])
    expect(shown.art).toEqual(art)
    expect(shown.visual).toEqual({ kind: 'mentions', rows: [{ label: 'Pauper', matches: 231, percent: 18 }] })
  })

  it('a player who stuck to one format gets no honourable mentions', () => {
    const art = { remote: null, local: null, custom: null }
    const shown = view({ kind: 'formats', favourite: { label: 'Pauper', matches: 40, percent: 100, art }, mentions: [] })
    expect(shown.visual).toBeNull()
  })

  it('when you play names the part of the day and says weekend or weekday', () => {
    const card = (period: 'morning' | 'afternoon' | 'evening' | 'night', weekends: boolean): WrappedCard => ({
      kind: 'whenYouPlay',
      peakWeekday: 1,
      peakHour: 23,
      busiestMonth: 8,
      byHour: Array(24).fill(0),
      period,
      weekends,
    })
    expect(view(card('morning', true)).title).toBe('Early bird')
    expect(view(card('afternoon', true)).title).toBe('Daylight duellist')
    expect(view(card('evening', true)).title).toBe('Prime-time player')
    const night = view(card('night', false))
    expect(night.title).toBe('Night owl')
    expect(night.scene).toBe('night')
    expect(night.lead).toContain('after 10pm')
    expect(night.facts.map((fact) => `${fact.label}: ${fact.value}`)).toEqual(['Your peak: 11pm on Tuesdays', 'Your week: Weekday regular'])
    expect(view(card('night', true)).facts[1].value).toBe('Weekend warrior')
  })

  it('a deck card brags about its archetype rank only in the top quarter', () => {
    const deck = TEST_BUNDLES.grinder.cards.find((card) => card.kind === 'deckOfYear')
    if (deck?.kind !== 'deckOfYear') throw new Error('no deck of the year')
    const ranked = (topPercent: number): WrappedCard => ({ ...deck, rank: { archetype: 'Boros Energy', topPercent } })
    expect(view(ranked(5)).badge).toBe("You're in the top 5% of MyMTGO Boros Energy players by matches played.")
    expect(view(ranked(25)).badge).not.toBeNull()
    expect(view(ranked(26)).badge).toBeNull()
    expect(view({ ...deck, rank: null }).badge).toBeNull()
  })

  it('the MVP card leads with its impact and says the win rate only at 50% or better', () => {
    const art = { remote: null, local: null, custom: null }
    const mvp = (winRate: number | null): WrappedCard => ({ kind: 'mvpCard', name: 'Ragavan, Nimble Pilferer', cast: 1, games: 1, winRate, impact: 1, art, image: art, runnersUp: [] })
    const shown = view(mvp(64))
    expect(shown.big).toBe('+1')
    expect(shown.lines).toEqual(['When you cast it you won 64% of games, 1 point above your usual.'])
    expect(shown.figures.map((figure) => `${figure.value} ${figure.label}`)).toEqual(['1 cast', '1 game', '64% won when cast'])
    const low = view(mvp(null))
    expect(low.lines).toEqual(['Casting it lifted your win rate by 1 point.'])
    expect(low.figures).toHaveLength(2)
  })

  it('the play or draw card names the side with its own art', () => {
    const play = view({ kind: 'playDraw', side: 'play', winRate: 61, games: 1530 })
    expect(play.title).toBe('First to the table')
    expect(play.big).toBe('61%')
    expect(play.bigLabel).toBe('of games won on the play')
    expect(play.credit).toContain('Seize the Initiative')
    const draw = view({ kind: 'playDraw', side: 'draw', winRate: 55, games: 1 })
    expect(draw.title).toBe('Patience pays')
    expect(draw.figures.map((figure) => `${figure.value} ${figure.label}`)).toEqual(['1 game on the draw'])
    expect(draw.credit).toContain('Divination')
  })

  it('a year with nothing listed still opens, with no panels', () => {
    const shown = cardView({ kind: 'magicYear' }, { ...BUNDLE, year: 2031 })
    expect(shown.title).toBe('A year of Magic')
    expect(shown.visual).toEqual({ kind: 'magicYear', year: { sets: [], winners: [], bans: [] } })
  })

  it('one Top 8 and one league trophy read in the singular', () => {
    const shown = view({ kind: 'trophies', leagueTrophies: 1, leagueRuns: 12, top8s: 1, bestFinish: null })
    expect(shown.title).toBe('You won 1 league!')
    expect(shown.big).toBe('12')
    expect(shown.bigLabel).toBe('league runs')
    expect(shown.figures.map((figure) => `${figure.value} ${figure.label}`)).toEqual(['1 league trophy', '1 Top 8'])
    expect(shown.credit).toContain('Arena of Glory')
  })

  it('a trophy cabinet with no wins still celebrates the runs', () => {
    const shown = view({ kind: 'trophies', leagueTrophies: 0, leagueRuns: 1, top8s: 0, bestFinish: null })
    expect(shown.title).toBe('Every run counts')
    expect(shown.bigLabel).toBe('league run')
    expect(shown.figures).toEqual([])
  })

  it('the best matchup never says they saw you coming', () => {
    const look = { art: { remote: null, local: null, custom: null }, colors: [], cards: [] }
    const shown = view({ kind: 'bestMatchup', archetype: 'Living End', matches: 24, wins: 19, winRate: 79, look })
    expect(shown.lines.join(' ')).toContain('They never saw you coming.')
  })

  it('the toughest rival card has no win rate to show', () => {
    const rival = BUNDLE.cards.find((card) => card.kind === 'toughestRival')
    expect(rival).toBeDefined()
    expect(rival && 'winRate' in rival).toBe(false)
  })
})
