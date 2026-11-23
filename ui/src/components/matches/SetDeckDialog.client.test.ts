import { beforeEach, describe, expect, it, vi } from 'vitest'
import { h, ref } from 'vue'
import type { HistoryDeckVersionView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  calls: [] as { tokens: string[]; version: number }[],
  answer: true,
}))

vi.mock('@/composables/useMatchEdit', async () => {
  const { ref } = await import('vue')
  return {
    useMatchEdit: () => ({
      saving: ref(false),
      setDeck: async (tokens: string[], version: number) => {
        state.calls.push({ tokens, version })
        return state.answer
      },
    }),
  }
})
const versions: HistoryDeckVersionView[] = [
  { id: 7, deckName: 'Qburn', archived: false, modifiedAt: '04/03/2026', format: 'Modern', currentAccount: true, limited: false },
  { id: 3, deckName: 'Qold', archived: true, modifiedAt: '01/01/2026', format: 'Modern', currentAccount: true, limited: false },
  { id: 4, deckName: 'Qtheirs', archived: false, modifiedAt: '01/01/2026', format: 'Modern', currentAccount: false, limited: false },
  { id: 5, deckName: 'Qpool', archived: false, modifiedAt: '01/01/2026', format: 'Limited', currentAccount: true, limited: true },
]
vi.mock('@/lib/queries', () => ({
  useHistoryDeckVersions: () => ({ data: ref(versions), isPending: ref(false) }),
}))
// The dialog and select as plain elements: the fake renderer has no portal.
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
vi.mock('@/components/ui/select', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Select: plain('select'),
    SelectContent: plain('select-content'),
    SelectGroup: plain('optgroup'),
    SelectItem: plain('option'),
    SelectLabel: plain('select-label'),
    SelectTrigger: plain('select-trigger'),
    SelectValue: plain('select-value'),
  }
})

const { default: SetDeckDialog } = await import('./SetDeckDialog.vue')

beforeEach(() => {
  state.calls = []
  state.answer = true
})

function byTestId(root: Parameters<typeof findAll>[0], id: string) {
  const [node] = findAll(root, (candidate) => candidate.props['data-testid'] === id)
  return node
}

describe('the "Set deck" dialog (issue #332)', () => {
  it('offers the Import page deck versions and sets the chosen one on the matches', async () => {
    const dialog = ref<{ openForMatches: (tokens: string[]) => void } | null>(null)
    const set: string[] = []
    const mounted = mountFake({
      render: () => h(SetDeckDialog, { ref: dialog, onDeckSet: () => set.push('set') }),
    })
    expect(mounted.warnings).toEqual([])
    dialog.value?.openForMatches(['q-1', 'q-2'])
    await mounted.settle()

    expect(textOf(mounted.root)).toContain('the 2 selected matches')
    const groups = findAll(mounted.root, (node) => node.tag === 'select-label').map((node) => textOf(node))
    expect(groups).toEqual(['Active Decks', 'Deleted Decks'])
    expect(textOf(mounted.root)).toContain('Qburn (Modern, 04/03/2026)')
    expect(textOf(mounted.root)).not.toContain('Qtheirs')
    expect(textOf(mounted.root)).not.toContain('Qpool')
    expect(byTestId(mounted.root, 'set-deck-save').props.disabled).toBe(true)

    ;(byTestId(mounted.root, 'set-deck-select').props['onUpdate:modelValue'] as (value: number) => void)(7)
    await mounted.settle()
    ;(byTestId(mounted.root, 'set-deck-save').props.onClick as () => void)()
    await mounted.settle()

    expect(state.calls).toEqual([{ tokens: ['q-1', 'q-2'], version: 7 }])
    expect(set).toEqual(['set'])
  })
})
