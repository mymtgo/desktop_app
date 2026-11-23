import { describe, expect, it } from 'vitest'
import type { LeagueTheme, OverlaySize } from '@/bindings'
import type { LeagueDisplay, LeagueOverlayState, LeagueStatus } from '@/bindings'
import { sampleEventState, sampleOverlayState } from '@/components/overlay/overlaySamples'
import { DEFAULT_DISPLAY } from '@/lib/leagueOverlayCard'
import { findAll, hasClass, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const { default: LeagueOverlayCard } = await import('./LeagueOverlayCard.vue')

const NOTHING: LeagueDisplay = { games: 'dots', event: false, format: false, title: false, progress: false, status: false, record: false, standing: false }

function card(status: LeagueStatus, size: OverlaySize, display: Partial<LeagueDisplay>, thumbnail = false): LeagueOverlayState {
  return sampleOverlayState(status, {
    art: 'https://cards.example/q.jpg',
    size,
    display: { ...DEFAULT_DISPLAY, ...display },
    thumbnail,
  })
}

const NOW = 1_800_000_000_000

function render(state: LeagueOverlayState): FakeNode {
  const mounted = mountFake(LeagueOverlayCard, { state, now: NOW })
  expect(mounted.warnings).toEqual([])
  return mounted.root
}

/** The card's outer element: its classes set the size, never the content. */
function outer(root: FakeNode): string {
  const node = root.children[0]
  return JSON.stringify(node.props.class)
}

describe('the league card', () => {
  it('keeps its size whatever is shown', () => {
    for (const size of ['full', 'compact'] as const) {
      const all = outer(render(card('in_game', size, {})))
      expect(outer(render(card('in_game', size, NOTHING)))).toBe(all)
      expect(outer(render(card('in_game', size, {}, true)))).toBe(all)
    }
  })

  it('shows no game number and letters when asked', () => {
    const text = textOf(render(card('in_game', 'full', { games: 'letters' })))
    expect(text).not.toContain('Game 3')
    expect(text).toContain('W')
    expect(text).toContain('L')
  })

  it('shows only the games and the record for the bare layout', () => {
    const text = textOf(render(card('in_game', 'full', { ...NOTHING, record: true, games: 'letters' })))
    // The status line starts with a zero-width space (its baseline when it shows only pips).
    expect(text.replace(/[\s\u200b]+/g, '')).toBe('W/L/-3-1')
  })

  it('shows nothing but the background with everything off', () => {
    expect(textOf(render(card('waiting', 'full', NOTHING))).trim()).toBe('')
  })

  it('keeps the indicators and record whole beside a long lead', () => {
    const root = render(card('in_game', 'compact', { games: 'letters' }))
    const lead = findAll(root, (node) => textOf(node) === 'Match 5/5' && node.tag === 'span')
    expect(lead.length).toBe(1)
    expect(hasClass(lead[0], 'truncate')).toBe(true)
    const games = findAll(root, (node) => hasClass(node, 'shrink-0') && textOf(node).includes('W'))
    expect(games.length).toBeGreaterThan(0)
    const record = findAll(root, (node) => node.tag === 'span' && textOf(node) === '3-1')
    expect(record.length).toBe(1)
    expect(hasClass(record[0], 'whitespace-nowrap')).toBe(true)
  })

  it('puts the art in a side thumbnail when asked, with no art behind the text', () => {
    const root = render(card('in_game', 'full', {}, true))
    expect(findAll(root, (node) => node.props['data-art'] === 'thumbnail').length).toBe(1)
    expect(findAll(root, (node) => node.props['data-art'] === 'background').length).toBe(0)
    const behind = render(card('in_game', 'full', {}))
    expect(findAll(behind, (node) => node.props['data-art'] === 'background').length).toBe(1)
  })

  it('renders the classic card for an unknown theme', () => {
    const state = card('in_game', 'full', {})
    const unknown = { ...state, theme: 'pixel' as LeagueTheme }
    expect(textOf(render(unknown))).toBe(textOf(render(state)))
  })

  it('gives the versus card the label input on the full size only', () => {
    const versus = (size: OverlaySize) => ({ ...card('in_game', size, {}), theme: 'versus' as LeagueTheme })
    const full = render(versus('full'))
    const inputs = findAll(full, (node) => node.tag === 'input')
    expect(inputs.length).toBe(1)
    expect(inputs[0].props.placeholder).toBe('Mono Green Tron')
    const compact = render(versus('compact'))
    expect(findAll(compact, (node) => node.tag === 'input')).toEqual([])
    expect(textOf(compact)).toContain('Mono Green Tron')
  })

  it('gives the versus card no label input with Archetype off, so it shows YOU', () => {
    const state = { ...card('in_game', 'full', { title: false }), theme: 'versus' as LeagueTheme }
    const root = render(state)
    expect(findAll(root, (node) => node.tag === 'input')).toEqual([])
    expect(textOf(root)).toContain('YOU')
  })

  it('shows the other event strip under the card only when the state has one, the card box unchanged', () => {
    for (const theme of ['classic', 'versus'] as const) {
      for (const size of ['full', 'compact'] as const) {
        const base = { art: null, size, theme, now: NOW }
        const plain = render(sampleEventState('in_game', base))
        const withStrip = render(sampleEventState('league_with_challenge', base))
        expect(findAll(plain, (node) => node.props['data-strip'] !== undefined)).toEqual([])
        const strips = findAll(withStrip, (node) => node.props['data-strip'] !== undefined)
        expect(strips.length, `${theme} ${size}`).toBe(1)
        expect(textOf(strips[0])).toContain(size === 'full' ? 'Challenge · Round 5 in ~14 min · 4-0' : 'Challenge: R5 ~14 min')
        const cardBox = (root: FakeNode) => JSON.stringify(root.children[0].children[0].props.style)
        expect(cardBox(withStrip)).toBe(cardBox(plain))
      }
    }
  })

  it('shows the deck title as plain text with readonly, in both themes', () => {
    for (const theme of ['classic', 'versus'] as LeagueTheme[]) {
      const state = { ...card('in_game', 'full', {}), theme }
      const editable = mountFake(LeagueOverlayCard, { state, now: NOW }).root
      expect(findAll(editable, (node) => node.tag === 'input').length, theme).toBe(1)
      const mounted = mountFake(LeagueOverlayCard, { state, now: NOW, readonly: true })
      expect(mounted.warnings).toEqual([])
      expect(findAll(mounted.root, (node) => node.tag === 'input'), theme).toEqual([])
      expect(textOf(mounted.root)).toContain('Mono Green Tron')
    }
  })

  it('shows the streamer label as plain text with readonly', () => {
    const base = card('in_game', 'full', {})
    const state = { ...base, deck: { ...base.deck!, label: 'My Brew' } }
    const text = textOf(mountFake(LeagueOverlayCard, { state, now: NOW, readonly: true }).root)
    expect(text).toContain('My Brew')
    expect(text).not.toContain('Mono Green Tron')
  })

  it('titles a Classic tournament card by the round, not editable, with the archetype in the line', () => {
    const state = sampleEventState('challenge_round', { art: null, size: 'full', now: NOW })
    const root = render(state)
    expect(findAll(root, (node) => node.tag === 'input')).toEqual([])
    const title = findAll(root, (node) => node.props['data-title'] === 'tournament')
    expect(title.map(textOf)).toEqual(['Round 4/6'])
    expect(textOf(root)).toContain('Mono Green Tron')
    const fallback = render(sampleEventState('challenge_round', { art: null, size: 'full', now: NOW, display: { ...DEFAULT_DISPLAY, progress: false } }))
    expect(findAll(fallback, (node) => node.tag === 'input').length).toBe(1)
  })
})
