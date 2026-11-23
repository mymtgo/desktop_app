import { describe, expect, it, vi } from 'vitest'
import type { MatchDetail } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('vue-router', () => ({ useRouter: () => ({ push: async () => {} }) }))
vi.mock('@/components/matches/SetArchetypeDialog.vue', () => stub('archetype-dialog'))
vi.mock('@/components/leagues/SetByYouBadge.vue', () => stub('set-by-you'))
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))

const { default: MatchHero } = await import('./MatchHero.vue')

const match = {
  id: 1,
  token: 'q-1',
  opponent: 'Qopp',
  format: 'Modern',
  outcome: 'win',
  wins: 2,
  losses: 0,
  startedAtMs: 1_800_000_000_000,
  endedAtMs: 1_800_000_600_000,
  corrected: false,
  deck: null,
  opponentArchetype: null,
  archetypePending: false,
  leagueName: null,
} as unknown as MatchDetail

function detectButton(detecting: boolean) {
  const mounted = mountFake(MatchHero, { match, detectable: true, detecting })
  expect(mounted.warnings).toEqual([])
  const [button] = findAll(mounted.root, (node) => node.props['aria-label'] === 'Detect')
  return button
}

describe('the match header Detect button', () => {
  it('spins and waits while the match is being detected', () => {
    const button = detectButton(true)
    expect(button.props.disabled).toBe(true)
    expect(findAll(button, (node) => hasClass(node, 'animate-spin'))).toHaveLength(1)
  })

  it('is still otherwise', () => {
    const button = detectButton(false)
    expect(button.props.disabled).toBe(false)
    expect(findAll(button, (node) => hasClass(node, 'animate-spin'))).toHaveLength(0)
  })
})

describe('the match header rating', () => {
  it('shows an Endstep ranked result as before to after with its signed change', () => {
    const mounted = mountFake(MatchHero, {
      match: { ...match, rating: { before: 1371, after: 1336, delta: -18 } },
    })
    expect(mounted.warnings).toEqual([])
    const [line] = findAll(mounted.root, (node) => node.props['data-testid'] === 'rating')
    expect(line).toBeDefined()
    expect(textOf(line).trim()).toBe('Rating 1371 to 1336 -18')
  })

  it('shows nothing without a rating', () => {
    const mounted = mountFake(MatchHero, { match })
    expect(findAll(mounted.root, (node) => node.props['data-testid'] === 'rating')).toHaveLength(0)
  })
})
