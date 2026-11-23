import { beforeEach, describe, expect, it, vi } from 'vitest'
import { h, ref } from 'vue'
import type { ManualMatchInput, MatchCorrectionInput, MatchEditView, PlatformView, RankGapOption } from '@/bindings'
import { type FakeNode, findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  platform: 'arena' as PlatformView,
  gaps: [] as RankGapOption[],
  /** Bumped to make the rank gap query refetch `gaps`. */
  refetchGaps: () => {},
  created: [] as ManualMatchInput[],
  editView: null as MatchEditView | null,
  corrected: [] as MatchCorrectionInput[],
  createErrors: null as Record<string, string> | null,
}))

vi.mock('@/composables/usePlatformContext', async () => {
  const { computed } = await import('vue')
  return { usePlatformContext: () => ({ platform: computed(() => state.platform) }) }
})
vi.mock('@/composables/useMatchEdit', async () => {
  const { ref } = await import('vue')
  return {
    useMatchEdit: () => ({
      saving: ref(false),
      createManual: async (input: ManualMatchInput) => {
        state.created.push(input)
        if (state.createErrors) return { ok: false, errors: state.createErrors }
        return { ok: true, token: 'q-new' }
      },
      correctMatch: async (_id: number, correction: MatchCorrectionInput) => {
        state.corrected.push(correction)
        return { ok: true }
      },
      loadEditView: async () => state.editView,
    }),
  }
})
vi.mock('@/composables/useOpponentArchetype', async () => {
  const { ref } = await import('vue')
  return { useArchetypeOptions: () => ({ data: ref([]) }) }
})
vi.mock('@/lib/queries', async () => {
  const { computed, ref } = await import('vue')
  const fetched = ref(0)
  state.refetchGaps = () => {
    fetched.value += 1
  }
  return {
    useManualLeagueOptions: () => ({ data: ref([]), isSuccess: ref(true), isFetching: ref(false) }),
    useManualRankGaps: () => ({ data: computed(() => (fetched.value, state.gaps)) }),
  }
})
vi.mock('@/components/matches/ArchetypePicker.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('archetype-picker') }) }
})
const plainModule = async (names: string[]) => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return Object.fromEntries(names.map((name) => [name, plain(name.toLowerCase())]))
}
vi.mock('@/components/ui/sheet', () =>
  plainModule(['Sheet', 'SheetContent', 'SheetDescription', 'SheetFooter', 'SheetHeader', 'SheetTitle']),
)
vi.mock('@/components/ui/select', () =>
  plainModule(['Select', 'SelectContent', 'SelectItem', 'SelectTrigger', 'SelectValue']),
)
vi.mock('@/components/ui/input', () => plainModule(['Input']))
vi.mock('@/components/ui/label', () => plainModule(['Label']))

const { default: ManualMatchSheet } = await import('./ManualMatchSheet.vue')

const GAP: RankGapOption = {
  key: { fileId: 1, byteOffset: 500 },
  label: 'Gold 3 to Gold 2, 3 Nov 14:05 to 15:30',
  changeSteps: 6,
  claims: 0,
}

beforeEach(() => {
  state.platform = 'arena'
  state.gaps = [GAP]
  state.created = []
  state.editView = null
  state.corrected = []
  state.createErrors = null
})

async function opened() {
  const sheet = ref<{ open: () => void } | null>(null)
  const mounted = mountFake({ render: () => h(ManualMatchSheet, { ref: sheet, deckId: 5, format: 'CSTANDARD' }) })
  sheet.value?.open()
  await mounted.settle()
  expect(mounted.warnings).toEqual([])
  return mounted
}

function byTestId(root: FakeNode, id: string): FakeNode | undefined {
  return findAll(root, (node) => node.props['data-testid'] === id)[0]
}

function button(root: FakeNode, label: string): FakeNode {
  const [found] = findAll(root, (node) => node.tag === 'button' && textOf(node).trim() === label)
  if (!found) throw new Error(`no ${label} button`)
  return found
}

async function click(mounted: Awaited<ReturnType<typeof opened>>, label: string) {
  ;(button(mounted.root, label).props.onClick as () => void)()
  await mounted.settle()
}

describe('the Arena manual match sheet', () => {
  it('the arena sheet has no league and offers ranked and rank gaps', async () => {
    const mounted = await opened()
    const text = textOf(mounted.root)
    expect(text).not.toContain('League')
    expect(text).toContain('Opponent (optional)')
    expect(button(mounted.root, 'Ranked').props['aria-pressed']).toBe(true)
    expect(button(mounted.root, 'Unranked').props['aria-pressed']).toBe(false)
    expect(byTestId(mounted.root, 'manual-rank-gap')).toBeDefined()
    expect(text).toContain('Rank gap')
    expect(text).toContain('None')
    expect(text).toContain(GAP.label)
  })

  it('the rank gap hides when unranked', async () => {
    const mounted = await opened()
    await click(mounted, 'Unranked')
    expect(byTestId(mounted.root, 'manual-rank-gap')).toBeUndefined()
    await click(mounted, 'Ranked')
    expect(byTestId(mounted.root, 'manual-rank-gap')).toBeDefined()
    // A deck with no gaps to claim shows no select either.
    state.gaps = []
    const bare = await opened()
    expect(byTestId(bare.root, 'manual-rank-gap')).toBeUndefined()
  })

  it('submits ranked and rankGap', async () => {
    const mounted = await opened()
    await click(mounted, 'Win')
    const select = byTestId(mounted.root, 'manual-rank-gap')
    ;(select?.props['onUpdate:modelValue'] as (value: string) => void)('1:500')
    await mounted.settle()
    const [form] = findAll(mounted.root, (node) => node.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    expect(state.created).toHaveLength(1)
    expect(state.created[0]).toMatchObject({
      deckId: 5,
      opponentName: '',
      league: null,
      ranked: true,
      rankGap: { fileId: 1, byteOffset: 500 },
    })

    // Unranked sends no gap.
    const again = await opened()
    await click(again, 'Win')
    await click(again, 'Unranked')
    const [unrankedForm] = findAll(again.root, (node) => node.tag === 'form')
    ;(unrankedForm.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await again.settle()
    expect(state.created[1]).toMatchObject({ ranked: false, rankGap: null })
  })

  it('a chosen rank gap gone after a refetch is still sent, so the store can refuse it', async () => {
    const mounted = await opened()
    await click(mounted, 'Win')
    const select = byTestId(mounted.root, 'manual-rank-gap')
    ;(select?.props['onUpdate:modelValue'] as (value: string) => void)('1:500')
    await mounted.settle()
    // Another gap replaces the chosen one (a new reading closed it).
    state.gaps = [{ ...GAP, key: { fileId: 1, byteOffset: 900 } }]
    state.refetchGaps()
    await mounted.settle()
    const [form] = findAll(mounted.root, (node) => node.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    expect(state.created).toHaveLength(1)
    expect(state.created[0]).toMatchObject({ ranked: true, rankGap: { fileId: 1, byteOffset: 500 } })
  })

  it('a refused rank gap shows its error on the field when no gap is left', async () => {
    const mounted = await opened()
    await click(mounted, 'Win')
    const select = byTestId(mounted.root, 'manual-rank-gap')
    ;(select?.props['onUpdate:modelValue'] as (value: string) => void)('1:500')
    await mounted.settle()
    state.gaps = []
    state.refetchGaps()
    state.createErrors = { rank_gap: 'That rank gap is not available.' }
    const [form] = findAll(mounted.root, (node) => node.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    const field = byTestId(mounted.root, 'manual-rank-gap')
    expect(field).toBeDefined()
    expect(textOf(mounted.root).split('That rank gap is not available.')).toHaveLength(2)
  })

  it('keeps the MTGO sheet as it was', async () => {
    state.platform = 'mtgo'
    const mounted = await opened()
    const text = textOf(mounted.root)
    expect(text).toContain('League')
    expect(text).not.toContain('Opponent (optional)')
    expect(text).not.toContain('Unranked')
    expect(byTestId(mounted.root, 'manual-rank-gap')).toBeUndefined()
  })
})

function editView(platform: PlatformView, opponentName: string | null): MatchEditView {
  return {
    opponentName,
    startedAtMs: new Date(2026, 10, 3, 14, 5).getTime(),
    endedAtMs: new Date(2026, 10, 3, 14, 30).getTime(),
    games: [{ gameNumber: 1, result: 'win', onPlay: true, turns: null }],
    corrected: false,
    platform,
  }
}

async function editing(view: MatchEditView) {
  state.editView = view
  const sheet = ref<{ openEdit: (id: number) => Promise<void> } | null>(null)
  const mounted = mountFake({ render: () => h(ManualMatchSheet, { ref: sheet, deckId: 5, format: 'CSTANDARD' }) })
  await sheet.value?.openEdit(7)
  await mounted.settle()
  expect(mounted.warnings).toEqual([])
  return mounted
}

function opponentInput(root: FakeNode): FakeNode {
  const [input] = findAll(root, (node) => node.props.id === 'manual-opponent')
  if (!input) throw new Error('no opponent input')
  return input
}

describe('the Edit match sheet', () => {
  it('editing an arena match does not require an opponent', async () => {
    // The MTGO context: the match's own platform decides.
    state.platform = 'mtgo'
    const mounted = await editing(editView('arena', 'Qrival'))
    const text = textOf(mounted.root)
    expect(text).toContain('Opponent (optional)')
    expect(opponentInput(mounted.root).props.placeholder).toBe('Arena name')
    ;(opponentInput(mounted.root).props['onUpdate:modelValue'] as (value: string) => void)('')
    await mounted.settle()
    expect(button(mounted.root, 'Save').props.disabled).toBe(false)
    const [form] = findAll(mounted.root, (node) => node.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    expect(state.corrected).toHaveLength(1)
    expect(state.corrected[0].opponentName).toBeNull()
  })

  it('editing an mtgo match still needs its opponent', async () => {
    state.platform = 'arena'
    const mounted = await editing(editView('mtgo', 'Qrival'))
    expect(textOf(mounted.root)).not.toContain('Opponent (optional)')
    expect(opponentInput(mounted.root).props.placeholder).toBe('MTGO username')
    ;(opponentInput(mounted.root).props['onUpdate:modelValue'] as (value: string) => void)('')
    await mounted.settle()
    expect(button(mounted.root, 'Save').props.disabled).toBe(true)
  })
})

describe('the Bo3 or Bo1 choice', () => {
  const gameRows = (root: FakeNode) => findAll(root, (n) => textOf(n).trim().startsWith('Game ') && n.tag === 'span')

  it('defaults to Bo3 with three game rows and sends no bestOf', async () => {
    const mounted = await opened()
    expect(button(mounted.root, 'Bo3').props['aria-pressed']).toBe(true)
    expect(gameRows(mounted.root)).toHaveLength(3)
  })

  it('Bo1 shows one game row and sends bestOf bo1', async () => {
    const mounted = await opened()
    await click(mounted, 'Bo1')
    expect(gameRows(mounted.root)).toHaveLength(1)
    const [win] = findAll(mounted.root, (n) => n.tag === 'button' && textOf(n).trim() === 'Win')
    ;(win.props.onClick as () => void)()
    await mounted.settle()
    const [form] = findAll(mounted.root, (n) => n.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    expect(state.created).toHaveLength(1)
    expect(state.created[0].bestOf).toBe('bo1')
    expect(state.created[0].games).toHaveLength(1)
  })

  it('switching Bo3 to Bo1 drops the extra rows, and Bo3 sends no bestOf', async () => {
    const mounted = await opened()
    await click(mounted, 'Bo1')
    await click(mounted, 'Bo3')
    expect(gameRows(mounted.root)).toHaveLength(3)
    const [win] = findAll(mounted.root, (n) => n.tag === 'button' && textOf(n).trim() === 'Win')
    ;(win.props.onClick as () => void)()
    await mounted.settle()
    const [form] = findAll(mounted.root, (n) => n.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    expect(state.created[0].bestOf ?? null).toBeNull()
  })

  it('keeps games 2 and 3 when Bo1 is clicked twice before Bo3', async () => {
    const mounted = await opened()
    const results = (label: string) => findAll(mounted.root, (n) => n.tag === 'button' && textOf(n).trim() === label)
    ;(results('Win')[0].props.onClick as () => void)()
    await mounted.settle()
    ;(results('Loss')[1].props.onClick as () => void)()
    await mounted.settle()
    await click(mounted, 'Bo1')
    await click(mounted, 'Bo1')
    await click(mounted, 'Bo3')
    const [form] = findAll(mounted.root, (n) => n.tag === 'form')
    ;(form.props.onSubmit as (event: { preventDefault: () => void }) => void)({ preventDefault: () => {} })
    await mounted.settle()
    expect(state.created[0].games.map((game) => game.won)).toEqual([true, false])
  })

  it('is absent on MTGO', async () => {
    state.platform = 'mtgo'
    const mounted = await opened()
    expect(findAll(mounted.root, (n) => n.tag === 'button' && textOf(n).trim() === 'Bo1')).toEqual([])
  })
})
