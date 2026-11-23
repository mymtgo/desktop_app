import { beforeEach, describe, expect, it, vi } from 'vitest'
import { h, ref } from 'vue'
import type { MatchLeagueTargetsView } from '@/bindings'
import { startedDayMonth } from '@/lib/leagues'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const OCT_11 = Date.UTC(2026, 9, 11, 12)
const state = vi.hoisted(() => ({
  calls: [] as [string, string | null, unknown][],
  view: null as MatchLeagueTargetsView | null,
}))

vi.mock('@/composables/useMatchEdit', async () => {
  const { ref } = await import('vue')
  return {
    useMatchEdit: () => ({
      saving: ref(false),
      moveToLeague: async (token: string, runKey: string | null, from: unknown) => {
        state.calls.push([token, runKey, from])
        return true
      },
    }),
  }
})
vi.mock('@/lib/queries', async () => {
  const { computed, ref } = await import('vue')
  return { useMatchLeagueTargets: () => ({ data: computed(() => state.view), isPending: ref(false) }) }
})
vi.mock('@/components/ui/dialog', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Dialog: plain('dialog'),
    DialogContent: plain('dialog-content'),
    DialogDescription: plain('dialog-description'),
    DialogFooter: plain('dialog-footer'),
    DialogHeader: plain('dialog-header'),
    DialogTitle: plain('dialog-title'),
  }
})

const { default: MoveToLeagueDialog } = await import('./MoveToLeagueDialog.vue')

const CURRENT = { key: 'q:old', name: 'Qold', startedAtMs: OCT_11 }

beforeEach(() => {
  state.calls = []
  state.view = {
    current: CURRENT,
    targets: [{ key: 'q:new', name: 'Qnew', startedAtMs: OCT_11, state: 'complete', manual: false, matchCount: 5 }],
  }
})

function byTestId(root: Parameters<typeof findAll>[0], id: string) {
  const [node] = findAll(root, (candidate) => candidate.props['data-testid'] === id)
  return node
}

async function opened() {
  const dialog = ref<{ openForMatch: (token: string) => void } | null>(null)
  const mounted = mountFake({ render: () => h(MoveToLeagueDialog, { ref: dialog }) })
  expect(mounted.warnings).toEqual([])
  dialog.value?.openForMatch('q-match')
  await mounted.settle()
  return mounted
}

describe('Move to league', () => {
  it('says where the match is and lists the leagues it may join, plus No league', async () => {
    const mounted = await opened()
    const text = textOf(mounted.root)
    expect(text).toContain(`Now in ${startedDayMonth(OCT_11)} league`)
    expect(text).toContain('Qnew')
    expect(text).toContain('Complete')
    expect(text).toContain('No league')
    expect(byTestId(mounted.root, 'move-submit').props.disabled).toBe(true)
  })

  it('moves the match to the chosen league', async () => {
    const mounted = await opened()
    ;(byTestId(mounted.root, 'move-target-q:new').props.onClick as () => void)()
    await mounted.settle()
    ;(byTestId(mounted.root, 'move-submit').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual([['q-match', 'q:new', CURRENT]])
  })

  it('takes it out of every league with No league', async () => {
    const mounted = await opened()
    ;(byTestId(mounted.root, 'move-target-none').props.onClick as () => void)()
    await mounted.settle()
    ;(byTestId(mounted.root, 'move-submit').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual([['q-match', null, CURRENT]])
  })

  it('offers no No league for a match in no league, and says when nothing fits', async () => {
    state.view = { current: null, targets: [] }
    const mounted = await opened()
    expect(byTestId(mounted.root, 'move-target-none')).toBeUndefined()
    expect(textOf(mounted.root)).toContain('No league can take this match')
  })
})
