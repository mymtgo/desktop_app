import { describe, expect, it, vi } from 'vitest'
import { h, ref } from 'vue'
import type { AvailableMatchView } from '@/bindings'
import { startedDayMonth } from '@/lib/leagues'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ linked: [] as [string, string[]][], toasts: [] as unknown[] }))

const OCT_11 = Date.UTC(2026, 9, 11, 12)
// The viewer's locale names the day ("11 Oct" in en-GB, "Oct 11" in en-US).
const DAY = startedDayMonth(OCT_11)
const rows: AvailableMatchView[] = [
  {
    token: 'q-a',
    startedAtMs: OCT_11,
    endedAtMs: OCT_11 + 1,
    result: 'W',
    opponentName: 'Qopponent',
    opponentArchetype: null,
    gameRecord: '2-0',
    currentLeague: { key: 'q:old', name: 'Qold', startedAtMs: OCT_11 },
  },
  {
    token: 'q-b',
    startedAtMs: OCT_11,
    endedAtMs: OCT_11 + 1,
    result: 'L',
    opponentName: 'Qother',
    opponentArchetype: null,
    gameRecord: '0-2',
    currentLeague: null,
  },
]

vi.mock('@tanstack/vue-query', () => ({ useQueryClient: () => ({ invalidateQueries: async () => {} }) }))
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  return { useLeagueAvailableMatches: () => ({ data: ref(rows), isPending: ref(false), dataUpdatedAt: ref(OCT_11) }) }
})
vi.mock('@/bindings', () => ({
  commands: {
    linkLeagueMatch: async (key: string, tokens: string[]) => {
      state.linked.push([key, tokens])
      return { status: 'ok', data: null }
    },
  },
}))
vi.mock('@/composables/useLeagueEdit', async () => {
  const { ref } = await import('vue')
  return {
    useLeagueEdit: () => ({
      saving: ref(false),
      run: async (_title: string, action: () => Promise<unknown>) => {
        await action()
        return true
      },
    }),
  }
})
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: (toast: unknown) => state.toasts.push(toast) }) }))
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

const { default: LinkMatchesDialog } = await import('./LinkMatchesDialog.vue')

type Opener = { openForRun: (key: string, matchCount: number, manual: boolean, limited?: boolean) => void }

async function opened(manual: boolean, matchCount: number, limited = false) {
  const dialog = ref<Opener | null>(null)
  const mounted = mountFake({ render: () => h(LinkMatchesDialog, { ref: dialog }) })
  expect(mounted.warnings).toEqual([])
  dialog.value?.openForRun('q:run', matchCount, manual, limited)
  await mounted.settle()
  return mounted
}

function pick(root: Parameters<typeof findAll>[0], token: string) {
  const [row] = findAll(root, (node) => node.props['data-token'] === token)
  ;(row.props.onClick as () => void)()
}

describe('Add matches', () => {
  it('shows where each match is now', async () => {
    const mounted = await opened(false, 5)
    const text = textOf(mounted.root)
    expect(text).toContain(`in ${DAY} league`)
    expect(text).toContain('no league')
  })

  it('caps a manual league at its free slots but not a log run', async () => {
    const manual = await opened(true, 4)
    expect(textOf(manual.root)).toContain('Select up to 1 match')
    const log = await opened(false, 5)
    expect(textOf(log.root)).toContain('Select matches played with this deck')
    pick(log.root, 'q-a')
    pick(log.root, 'q-b')
    await log.settle()
    expect(textOf(log.root)).toContain('2 selected')
  })

  it('describes a limited run\'s matches as of its set', async () => {
    const limited = await opened(false, 3, true)
    expect(textOf(limited.root)).toContain('Select matches of this set')
    expect(textOf(limited.root)).not.toContain('this deck')
  })

  it('moves one match and names where it came from', async () => {
    state.linked = []
    state.toasts = []
    const mounted = await opened(false, 5)
    pick(mounted.root, 'q-a')
    await mounted.settle()
    const [submit] = findAll(mounted.root, (node) => node.props['data-testid'] === 'link-matches-submit')
    ;(submit.props.onClick as () => void)()
    await mounted.settle()
    expect(state.linked).toEqual([['q:run', ['q-a']]])
    expect(state.toasts).toEqual([
      { type: 'success', title: 'Match added', message: `It was in the ${DAY} league.` },
    ])
  })
})
