import { describe, expect, it, vi } from 'vitest'
import { ref, unref, type MaybeRef } from 'vue'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  event: { picksRecorded: false, setLabel: null, draftToken: null } as Record<string, unknown>,
  draft: { redirectToDeck: false, selectedOrdinal: null, review: null } as Record<string, unknown>,
  deckEnabled: [] as MaybeRef<boolean>[],
}))

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, props: ['groups'], setup: (_p, { attrs }) => () => h(tag, { ...attrs }) }) }
})

vi.mock('vue-router', () => ({
  useRoute: () => ({ query: {} }),
  useRouter: () => ({ replace: () => {} }),
}))
vi.mock('@/pages/limited/useLimitedKey', () => ({ useLimitedKey: () => ref('arena:1') }))
vi.mock('@/lib/queries', () => ({
  useLimitedEvent: () => ({ data: ref(state.event) }),
  useLimitedDraft: () => ({ data: ref(state.draft), error: ref(null) }),
  useLimitedDeck: (_key: unknown, enabled: MaybeRef<boolean> = true) => {
    state.deckEnabled.push(enabled)
    return { data: ref({ pool: { groups: [] }, cards: {} }) }
  },
  useLimitedCrossDrafts: () => ({ data: ref(undefined) }),
}))
vi.mock('@/lib/limitedPages', () => ({ wholePool: () => [{ catalogId: 1 }] }))
vi.mock('@/components/limited/CardImageGroups.vue', () => stub('card-image-groups'))
vi.mock('@/components/limited/CrossDraftCard.vue', () => stub('x'))
vi.mock('@/components/limited/ManaPips.vue', () => stub('x'))
vi.mock('@/components/limited/PickDetail.vue', () => stub('x'))
vi.mock('@/components/limited/PickNoteEditor.vue', () => stub('x'))
vi.mock('@/components/limited/SignalsPanel.vue', () => stub('x'))

const { default: DraftPage } = await import('./DraftPage.vue')

describe('the Draft page', () => {
  it('says Arena logs no picks and keeps the pool when none are recorded', () => {
    const mounted = mountFake(DraftPage)
    expect(textOf(mounted.root)).toContain('Arena does not log picks for this draft.')
    expect(findAll(mounted.root, (node) => node.tag === 'card-image-groups')).toHaveLength(1)
  })

  it('keeps the MTGO empty state when picks are recorded but the review is missing', () => {
    state.event = { ...state.event, picksRecorded: true }
    const mounted = mountFake(DraftPage)
    const text = textOf(mounted.root)
    expect(text).not.toContain('Arena does not log picks')
    expect(text).toContain('No draft recorded for this event.')
  })

  it('reads the deck only when picks are missing', () => {
    state.event = { ...state.event, picksRecorded: true }
    state.deckEnabled = []
    mountFake(DraftPage)
    expect(state.deckEnabled.map((enabled) => unref(enabled))).toEqual([false])

    state.event = { ...state.event, picksRecorded: false }
    state.deckEnabled = []
    mountFake(DraftPage)
    expect(state.deckEnabled.map((enabled) => unref(enabled))).toEqual([true])
  })
})
