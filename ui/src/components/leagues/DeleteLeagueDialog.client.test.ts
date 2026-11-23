import { describe, expect, it, vi } from 'vitest'
import { h, ref } from 'vue'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ calls: [] as [string, string][] }))

vi.mock('@/composables/useLeagueManage', async () => {
  const { ref } = await import('vue')
  return {
    useLeagueManage: () => ({
      saving: ref(false),
      deleteLeague: async (key: string, matches: string) => {
        state.calls.push([key, matches])
        return true
      },
    }),
  }
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

const { default: DeleteLeagueDialog } = await import('./DeleteLeagueDialog.vue')

type Opener = { openForRun: (key: string, matchCount: number) => void }

function byTestId(root: Parameters<typeof findAll>[0], id: string) {
  const [node] = findAll(root, (candidate) => candidate.props['data-testid'] === id)
  return node
}

async function opened(matchCount: number) {
  const dialog = ref<Opener | null>(null)
  const mounted = mountFake({ render: () => h(DeleteLeagueDialog, { ref: dialog }) })
  expect(mounted.warnings).toEqual([])
  dialog.value?.openForRun('q:1', matchCount)
  await mounted.settle()
  return mounted
}

describe('the delete league dialog', () => {
  it('asks plainly for a league with no matches', async () => {
    state.calls = []
    const mounted = await opened(0)
    expect(textOf(mounted.root)).toContain('Delete this league? You can restore it from Settings, Archives.')
    expect(byTestId(mounted.root, 'delete-keep')).toBeUndefined()
    ;(byTestId(mounted.root, 'delete-league-confirm').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual([['q:1', 'keep']])
  })

  it('keeps the matches by default and deletes them when chosen', async () => {
    state.calls = []
    const mounted = await opened(3)
    const text = textOf(mounted.root)
    expect(text).toContain('What should happen to its 3 matches?')
    expect(text).toContain('Keep them as non-league matches')
    expect(text).toContain('Delete them too')
    expect(byTestId(mounted.root, 'delete-keep').props.checked).toBe(true)
    ;(byTestId(mounted.root, 'delete-too').props.onChange as () => void)()
    await mounted.settle()
    expect(byTestId(mounted.root, 'delete-too').props.checked).toBe(true)
    ;(byTestId(mounted.root, 'delete-league-confirm').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual([['q:1', 'delete']])
  })
})
