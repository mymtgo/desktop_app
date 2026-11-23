import { afterEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import type { OverlaySize } from '@/bindings'
import type { LeagueOverlayState, LeagueStatus } from '@/bindings'
import { ANIMATION_MS } from '@/lib/versusCard'
import { sampleOverlayState } from '@/components/overlay/overlaySamples'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: VersusLeagueCard } = await import('./VersusLeagueCard.vue')

const NOW = 1_800_000_000_000

function versus(status: LeagueStatus, size: OverlaySize = 'full'): LeagueOverlayState {
  return sampleOverlayState(status, { art: null, size, theme: 'versus' })
}

describe('the versus card', () => {
  it('shows both sides, life and the match in game', () => {
    const mounted = mountFake(VersusLeagueCard, { now: NOW, state: versus('in_game') })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    for (const part of ['VS', 'MATCH 5/5', 'Mono Green Tron', 'Dimir Tempo', '14', '4', 'MODERN · 3-1']) {
      expect(text).toContain(part)
    }
  })

  it('sets the record apart in the bottom line and the compact middle', () => {
    for (const size of ['full', 'compact'] as const) {
      const records = findAll(mountFake(VersusLeagueCard, { now: NOW, state: versus('in_game', size) }).root, (node) => hasClass(node, 'record'))
      expect(records.map(textOf)).toEqual(['3-1'])
    }
  })

  it('steps a long archetype name down a size', () => {
    const base = versus('in_game')
    const long = { ...base, deck: { ...base.deck!, archetype: 'Dimir Murktide Control' } }
    const names = findAll(mountFake(VersusLeagueCard, { now: NOW, state: long }).root, (node) => hasClass(node, 'name') && hasClass(node, 'you'))
    expect(names.length).toBe(1)
    expect(hasClass(names[0], 'fit-longer')).toBe(true)
  })

  it('sizes and shows the name from the label when one is set', () => {
    const base = versus('in_game')
    const labelled = (archetype: string, label: string) => ({ ...base, deck: { ...base.deck!, archetype, label } })
    const yourNames = (state: LeagueOverlayState) =>
      findAll(mountFake(VersusLeagueCard, { now: NOW, state }).root, (node) => hasClass(node, 'name') && hasClass(node, 'you'))
    const long = yourNames(labelled('Tron', 'My Spicy Sultai Brew'))
    expect(hasClass(long[0], 'fit-longer')).toBe(true)
    expect(textOf(long[0])).toBe('My Spicy Sultai Brew')
    const short = yourNames(labelled('Dimir Murktide Control', 'Murktide'))
    expect(hasClass(short[0], 'fit-normal')).toBe(true)
    expect(textOf(short[0])).toBe('Murktide')
    const compact = mountFake(VersusLeagueCard, { now: NOW, state: { ...labelled('Tron', 'Murktide'), size: 'compact' } }).root
    expect(textOf(compact)).toContain('Murktide')
  })

  it('says CHALLENGER when the opponent is not known', () => {
    for (const size of ['full', 'compact'] as const) {
      const base = versus('in_game', size)
      const unknown = { ...base, match: { ...base.match!, opponentArchetype: undefined } }
      expect(textOf(mountFake(VersusLeagueCard, { now: NOW, state: unknown }).root)).toContain('CHALLENGER')
    }
  })

  it('names the opponent in both sizes, stepping a long archetype down a size', () => {
    for (const size of ['full', 'compact'] as const) {
      const base = versus('in_game', size)
      const named = (opponentArchetype: string) => ({ ...base, match: { ...base.match!, opponentArchetype } })
      const opponentNames = (state: LeagueOverlayState) =>
        findAll(mountFake(VersusLeagueCard, { now: NOW, state }).root, (node) => (hasClass(node, 'name') || hasClass(node, 'rname')) && hasClass(node, 'opponent'))
      const short = opponentNames(named('Boros Energy'))
      expect(short.length).toBe(1)
      expect(textOf(short[0])).toBe('Boros Energy')
      expect(hasClass(short[0], 'fit-normal')).toBe(true)
      const long = opponentNames(named('Dimir Murktide Control'))
      expect(textOf(long[0])).toBe('Dimir Murktide Control')
      expect(hasClass(long[0], 'fit-longer')).toBe(true)
    }
  })

  it('calls out the waiting state', () => {
    const text = textOf(mountFake(VersusLeagueCard, { now: NOW, state: versus('waiting') }).root)
    expect(text).toContain('A CHALLENGER APPROACHES')
  })

  it('plays nothing on mount', () => {
    const mounted = mountFake(VersusLeagueCard, { now: NOW, state: versus('in_game') })
    expect(findAll(mounted.root, (node) => node.props['data-animation'] !== undefined)).toEqual([])
  })

  it('plays FIGHT when a game starts', async () => {
    const mounted = mountFake(VersusLeagueCard, { now: NOW, state: versus('waiting') })
    mounted.setProps({ state: versus('in_game') })
    await nextTick()
    const playing = findAll(mounted.root, (node) => node.props['data-animation'] === 'fight')
    expect(playing.length).toBe(1)
    expect(textOf(playing[0])).toContain('GAME 3')
    expect(textOf(playing[0])).toContain('FIGHT!')
  })

  it('keeps the compact sideboarding banner in the bottom row', () => {
    const mounted = mountFake(VersusLeagueCard, { now: NOW, state: versus('sideboarding', 'compact') })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('CHALLENGER')
    expect(text).toContain('GAME 2')
    const rows = findAll(mounted.root, (node) => node.props['data-row'] === 'compact')
    expect(rows.length).toBe(1)
    expect(textOf(rows[0])).toContain('GAME 2')
    expect(textOf(rows[0])).toContain('CHALLENGER')
  })

  it('renders the compact layout', () => {
    const text = textOf(mountFake(VersusLeagueCard, { now: NOW, state: versus('in_game', 'compact') }).root)
    expect(text).toContain('M5/5')
  })

  it('leaves a damage chunk on each bar that dropped in one push', async () => {
    const base = versus('in_game')
    const at = (you: number, opponent: number): LeagueOverlayState => ({
      ...base,
      match: { ...base.match!, life: { you, opponent, startYou: 20, startOpponent: 20 } },
    })
    const mounted = mountFake(VersusLeagueCard, { now: NOW, state: at(20, 20) })
    mounted.setProps({ state: at(14, 10) })
    await nextTick()
    const ghosts = findAll(mounted.root, (node) => hasClass(node, 'ghost'))
    expect(ghosts.length).toBe(2)
    const sides = ghosts.map((ghost) => (hasClass(ghost.parent!, 'you') ? 'you' : 'opponent'))
    expect(sides.sort()).toEqual(['opponent', 'you'])
  })

  describe('the callout queue', () => {
    afterEach(() => vi.useRealTimers())

    function inGameWith(games: Array<{ won: boolean | null }>): LeagueOverlayState {
      const base = versus('in_game')
      return {
        ...base,
        match: {
          number: 5,
          games,
          gamesWon: games.filter((g) => g.won === true).length,
          gamesLost: games.filter((g) => g.won === false).length,
          life: { you: 20, opponent: 20, startYou: 20, startOpponent: 20 },
        },
      }
    }

    function sideboardingWith(games: Array<{ won: boolean | null }>): LeagueOverlayState {
      const base = versus('sideboarding')
      return { ...base, match: { ...inGameWith(games).match!, life: null } }
    }

    function playing(mounted: ReturnType<typeof mountFake>): string[] {
      return findAll(mounted.root, (node) => node.props['data-animation'] !== undefined).map((node) => String(node.props['data-animation']))
    }

    it('plays a burst of pushes one after another, in order', async () => {
      vi.useFakeTimers()
      const mounted = mountFake(VersusLeagueCard, { now: NOW, state: inGameWith([{ won: null }]) })
      // Game 1 lost, sideboarding, then game 2 starts, all inside the K.O.
      mounted.setProps({ state: sideboardingWith([{ won: false }]) })
      await nextTick()
      mounted.setProps({ state: inGameWith([{ won: false }, { won: null }]) })
      await nextTick()
      expect(playing(mounted)).toEqual(['ko'])
      vi.advanceTimersByTime(ANIMATION_MS.ko)
      await nextTick()
      expect(playing(mounted)).toEqual(['round'])
      vi.advanceTimersByTime(ANIMATION_MS.round)
      await nextTick()
      expect(playing(mounted)).toEqual(['fight'])
      vi.advanceTimersByTime(ANIMATION_MS.fight)
      await nextTick()
      expect(playing(mounted)).toEqual([])
      expect(mounted.warnings).toEqual([])
    })

    it('clears its timer when unmounted', async () => {
      vi.useFakeTimers()
      const mounted = mountFake(VersusLeagueCard, { now: NOW, state: versus('waiting') })
      mounted.setProps({ state: versus('in_game') })
      await nextTick()
      expect(playing(mounted)).toEqual(['fight'])
      expect(vi.getTimerCount()).toBe(1)
      mounted.unmount()
      expect(vi.getTimerCount()).toBe(0)
    })
  })

  describe('after a K.O.', () => {
    afterEach(() => vi.useRealTimers())

    function inGame(games: Array<{ won: boolean | null }>, life: number): LeagueOverlayState {
      const base = versus('in_game')
      return {
        ...base,
        match: {
          number: 5,
          games,
          gamesWon: games.filter((g) => g.won === true).length,
          gamesLost: games.filter((g) => g.won === false).length,
          life: { you: life, opponent: 20, startYou: 20, startOpponent: 20 },
        },
      }
    }

    it('keeps the loser at zero after the K.O. while still in game', async () => {
      vi.useFakeTimers()
      const mounted = mountFake(VersusLeagueCard, { now: NOW, state: inGame([{ won: null }], 7) })
      mounted.setProps({ state: inGame([{ won: false }], 7) })
      await nextTick()
      vi.advanceTimersByTime(ANIMATION_MS.ko + 50)
      await nextTick()
      const text = textOf(mounted.root)
      expect(findAll(mounted.root, (node) => node.props['data-animation'] !== undefined)).toEqual([])
      expect(text).toContain('0')
      expect(text).not.toContain('7')
    })

    it('clears the zero when the next game starts', async () => {
      vi.useFakeTimers()
      const mounted = mountFake(VersusLeagueCard, { now: NOW, state: inGame([{ won: null }], 7) })
      mounted.setProps({ state: inGame([{ won: false }], 7) })
      await nextTick()
      vi.advanceTimersByTime(ANIMATION_MS.ko + 50)
      mounted.setProps({ state: inGame([{ won: false }, { won: null }], 20) })
      await nextTick()
      const text = textOf(mounted.root)
      expect(text.split('20').length - 1).toBe(2)
    })
  })
})
