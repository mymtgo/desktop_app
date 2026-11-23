import { describe, expect, it, vi } from 'vitest'
import { h } from 'vue'
import type { MatchSummary } from '@/bindings'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub, useRouter: () => ({ push: async () => {} }) }
})
vi.mock('@/bindings', () => ({ commands: {} }))
vi.mock('@/composables/useMatchEdit', async () => {
  const { ref } = await import('vue')
  return { useMatchEdit: () => ({ saving: ref(false), hide: async () => true }) }
})
vi.mock('@/composables/useOpponentArchetype', () => ({
  useSetOpponentArchetype: () => ({ setArchetype: async () => true }),
}))
// The row menu as a plain wrapper around its row.
vi.mock('@/components/matches/MatchRowContextMenu.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: (_, { slots }) => () => h('row-menu', slots.default?.()) }) }
})
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/matches/MatchNotesDialog.vue', () => stub('notes-dialog'))
vi.mock('@/components/matches/SetArchetypeDialog.vue', () => stub('archetype-dialog'))
vi.mock('@/components/matches/SetDeckDialog.vue', () => stub('deck-dialog'))
vi.mock('@/components/matches/MoveToLeagueDialog.vue', () => stub('move-dialog'))
vi.mock('@/components/matches/ManualMatchSheet.vue', () => stub('manual-sheet'))
vi.mock('@/components/ui/checkbox', async () => {
  const { defineComponent, h } = await import('vue')
  return { Checkbox: defineComponent({ inheritAttrs: true, setup: () => () => h('checkbox') }) }
})
vi.mock('@/components/ui/dialog', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) => defineComponent({ setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Dialog: plain('dialog'),
    DialogContent: plain('dialog-content'),
    DialogDescription: plain('dialog-description'),
    DialogFooter: plain('dialog-footer'),
    DialogHeader: plain('dialog-header'),
    DialogTitle: plain('dialog-title'),
  }
})

const { default: MatchesTable } = await import('./MatchesTable.vue')

function row(token: string): MatchSummary {
  return {
    id: 1,
    token,
    opponent: 'Qopp',
    format: 'Modern',
    matchType: null,
    state: 'complete',
    outcome: 'win',
    tentativeOutcome: null,
    wins: 2,
    losses: 0,
    needsAttention: false,
    attentionReason: null,
    startedAtMs: 1_800_000_000_000,
    endedAtMs: 1_800_000_600_000,
    revision: 1,
    notes: null,
    gameResults: [],
    manual: false,
    limited: false,
    opponentArchetype: { id: 4, name: 'Qburn', colors: ['R'], commanderCards: [] },
  } as unknown as MatchSummary
}

function byTestId(root: FakeNode, id: string): FakeNode | undefined {
  return findAll(root, (node) => node.props['data-testid'] === id)[0]
}

function mount(detectable: boolean, detecting: string[] = []) {
  const detected: string[][] = []
  const mounted = mountFake({
    render: () =>
      h(MatchesTable, {
        rows: [row('q-1'), row('q-2')],
        selectable: true,
        detectable,
        detecting,
        onDetect: (tokens: string[]) => detected.push(tokens),
      }),
  })
  return { mounted, detected }
}

async function selectAll(mounted: ReturnType<typeof mountFake>) {
  const [all] = findAll(mounted.root, (node) => node.props['aria-label'] === 'Select all')
  ;(all.props['onUpdate:modelValue'] as (checked: boolean) => void)(true)
  await mounted.settle()
}

describe('the matches table selection bar', () => {
  it('detects the archetypes of the selected matches again', async () => {
    const { mounted, detected } = mount(true)
    // RouterLink is registered globally in the app, and no router is installed here.
    expect(mounted.warnings.filter((warning) => !warning.includes('RouterLink'))).toEqual([])
    await selectAll(mounted)
    expect(textOf(mounted.root)).toContain('2 selected')

    const button = byTestId(mounted.root, 'bulk-detect')
    expect(button).toBeDefined()
    ;(button!.props.onClick as () => void)()
    await mounted.settle()

    expect(detected).toEqual([['q-1', 'q-2']])
    expect(textOf(mounted.root)).not.toContain('2 selected')
  })

  it('detects in page order, not the order the rows were ticked', async () => {
    const { mounted, detected } = mount(true)
    const boxes = findAll(mounted.root, (node) => node.props['aria-label'] === 'Select match')
    for (const index of [1, 0]) {
      ;(boxes[index].props['onUpdate:modelValue'] as (checked: boolean) => void)(true)
      await mounted.settle()
    }
    ;(byTestId(mounted.root, 'bulk-detect')!.props.onClick as () => void)()
    expect(detected).toEqual([['q-1', 'q-2']])
  })

  it('offers no detection where the page does not detect', async () => {
    const { mounted } = mount(false)
    await selectAll(mounted)
    expect(textOf(mounted.root)).toContain('2 selected')
    expect(byTestId(mounted.root, 'bulk-detect')).toBeUndefined()
  })
})

describe('the matches table while detecting', () => {
  it('shows a spinner in place of the archetype of each match being detected', () => {
    const { mounted } = mount(true, ['q-2'])
    expect(mounted.warnings.filter((warning) => !warning.includes('RouterLink'))).toEqual([])
    const cells = findAll(mounted.root, (node) => node.props['data-cell'] === 'archetype')
    expect(cells.map((cell) => textOf(cell).trim())).toEqual(['Qburn', ''])
    const spinners = findAll(cells[1], (node) => node.props['aria-label'] === 'Detecting')
    expect(spinners).toHaveLength(1)
    expect(findAll(cells[0], (node) => node.props['aria-label'] === 'Detecting')).toHaveLength(0)
  })
})

describe('the matches table in Bo1 mode', () => {
  function headers(singleResult: boolean) {
    const mounted = mountFake({
      render: () => h(MatchesTable, { rows: [row('q-1')], singleResult, sortable: true }),
    })
    return findAll(mounted.root, (node) => node.tag === 'th').map((node) => textOf(node).trim())
  }

  it('shows one Result column and no game columns', () => {
    const labels = headers(true)
    expect(labels.filter((label) => label === 'Result')).toHaveLength(1)
    expect(labels).not.toContain('Game 1')
    expect(labels).not.toContain('Game 3')
  })

  it('shows the game columns when off', () => {
    const labels = headers(false)
    expect(labels).toContain('Game 1')
    expect(labels).toContain('Game 3')
  })
})

describe('the matches table Arena labels', () => {
  // `matchType` arrives as the display label (`match_type_label` in app/src/summary.rs).
  function mount(rows: Partial<MatchSummary>[], props: Record<string, unknown> = {}) {
    return mountFake({
      render: () => h(MatchesTable, { rows: rows.map((change, index) => ({ ...row(`q-${index}`), ...change })), ...props }),
    })
  }
  function cells(rows: Partial<MatchSummary>[], name: string, props: Record<string, unknown> = {}) {
    return findAll(mount(rows, props).root, (node) => node.props['data-cell'] === name)
  }
  const ranked = { platform: 'arena', bestOf: 'bo1', matchType: 'Ranked' } as const
  const goldRank = { class: 'Gold', level: 2, step: 3, mythicPercentile: null, mythicPlacement: null, label: 'Gold 2' }
  function headerTexts(rows: Partial<MatchSummary>[]): string[] {
    return findAll(mount(rows).root, (node) => node.tag === 'th').map((node) => textOf(node).trim())
  }

  it('shows an Endstep row its rating after with the signed change', () => {
    const endstep = { platform: 'endstep', bestOf: 'bo3', matchType: 'Ranked' } as const
    const rows: Partial<MatchSummary>[] = [
      { ...endstep, rating: { before: 1371, after: 1336, delta: -35 } },
      { ...endstep, rating: { before: 1336, after: 1348, delta: 12 } },
      { ...endstep, matchType: 'Unranked' },
      { ...endstep, rating: null },
    ]
    expect(headerTexts(rows)).toContain('Rank')
    expect(cells(rows, 'rank').map((node) => textOf(node).trim())).toEqual(['1336 -35', '1348 +12', 'Casual', 'Ranked'])
  })

  it('shows a Bo column with Bo1 and Bo3 app-wide', () => {
    const rows: Partial<MatchSummary>[] = [ranked, { ...ranked, bestOf: 'bo3' }]
    expect(headerTexts(rows)).toContain('Bo')
    expect(cells(rows, 'bo').map((node) => textOf(node).trim())).toEqual(['Bo1', 'Bo3'])
  })

  it('leaves the Bo column out where the page already shows the kind', () => {
    const table = mount([ranked], { hideKind: true })
    expect(findAll(table.root, (node) => node.tag === 'th').map((node) => textOf(node).trim())).not.toContain('Bo')
    expect(findAll(table.root, (node) => node.props['data-cell'] === 'bo')).toEqual([])
  })

  it('shows only the rank icon, named in its tooltip, for a ranked match with a rank', () => {
    const rows: Partial<MatchSummary>[] = [{ ...ranked, rank: goldRank }]
    expect(headerTexts(rows)).toContain('Rank')
    const [cell] = cells(rows, 'rank')
    expect(textOf(cell).trim()).toBe('')
    const [icon] = findAll(cell, (node) => node.tag === 'img' && node.props['data-testid'] === 'rank-icon')
    expect(icon.props.alt).toBe('Gold 2')
    expect(icon.props.title).toBe('Gold 2')
  })

  it('shows Ranked when a ranked match has no reading, so it is not taken for casual', () => {
    const [cell] = cells([{ ...ranked, rank: null }], 'rank')
    expect(textOf(cell).trim()).toBe('Ranked')
    expect(findAll(cell, (node) => node.tag === 'img')).toHaveLength(0)
  })

  it('leaves the cell empty for a casual match', () => {
    const [cell] = cells([{ ...ranked, matchType: 'Unranked' }], 'rank')
    expect(textOf(cell).trim()).toBe('')
    expect(findAll(cell, (node) => node.tag === 'img')).toHaveLength(0)
  })

  it('has no Type column any more', () => {
    expect(headerTexts([ranked])).not.toContain('Type')
  })

  it('has no Rank or Bo column for MTGO rows', () => {
    const rows: Partial<MatchSummary>[] = [{ platform: 'mtgo', bestOf: null, matchType: 'Modern' }]
    expect(headerTexts(rows)).not.toContain('Rank')
    expect(headerTexts(rows)).not.toContain('Bo')
  })
})
