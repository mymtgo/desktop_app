import { describe, expect, it, vi } from 'vitest'
import type { RunCardView } from '@/bindings'
import { findAll, mountFake } from '@/replay/replayTestHarness'

vi.mock('@/components/leagues/LeagueCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: { league: Object, now: Number, compact: Boolean },
      setup: (props) => () => h('leagues-card', { ...props }),
    }),
  }
})

const { default: LeagueCard } = await import('./LeagueCard.vue')

const CONSTRUCTED = { key: 'q-run:1', limited: false } as unknown as RunCardView
const LIMITED = { key: 'q-draft:2', limited: true } as unknown as RunCardView

describe('the League in progress card', () => {
  for (const [kind, league] of [
    ['constructed', CONSTRUCTED],
    ['limited', LIMITED],
  ] as const) {
    it(`is the Leagues page run card, compact, for a ${kind} run`, () => {
      const mounted = mountFake(LeagueCard, { league, now: 1234 })
      expect(mounted.warnings).toEqual([])
      const cards = findAll(mounted.root, (node) => node.tag === 'leagues-card')
      expect(cards).toHaveLength(1)
      expect((cards[0].props.league as RunCardView).key).toBe(league.key)
      expect(cards[0].props.now).toBe(1234)
      expect(cards[0].props.compact).toBe(true)
    })
  }
})
