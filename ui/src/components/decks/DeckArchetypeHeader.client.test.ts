import { describe, expect, it, vi } from 'vitest'
import type { ArchetypeHeaderView, PlatformView } from '@/bindings'
import { mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ platform: 'mtgo' as PlatformView }))

vi.mock('vue-router', () => ({ useRouter: () => ({ push: () => {} }) }))
vi.mock('@/composables/usePlatformContext', async () => {
  const { computed } = await import('vue')
  return { usePlatformContext: () => ({ platform: computed(() => state.platform) }) }
})
vi.mock('@/components/mana/ManaColors.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('mana-colors') }) }
})
vi.mock('@/components/SegmentedControl.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('segmented') }) }
})

const { default: DeckArchetypeHeader } = await import('./DeckArchetypeHeader.vue')

const header: ArchetypeHeaderView = {
  archetype: { id: 1, name: 'Qburn', colors: [], format: 'CMODERN', isFallback: false },
  deckCount: 2,
  record: { wins: 3, losses: 1, draws: 0, total: 4, winRate: 75, label: '3 - 1' },
  best: null,
  worst: null,
}

describe('the archetype header record', () => {
  it('is labelled All on Arena', () => {
    state.platform = 'arena'
    const mounted = mountFake(DeckArchetypeHeader, { header })
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toContain('All win rate')
  })

  it('stays plain win rate on MTGO', () => {
    state.platform = 'mtgo'
    const text = textOf(mountFake(DeckArchetypeHeader, { header }).root)
    expect(text).toContain('win rate')
    expect(text).not.toContain('All win rate')
  })
})
