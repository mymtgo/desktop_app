import { describe, expect, it } from 'vitest'
import { vi } from 'vitest'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

// reka-ui's menu as plain elements: the fake renderer has no portal. An
// item keeps its `onSelect` listener as a prop, so a test can choose it.
vi.mock('reka-ui', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    ContextMenuRoot: plain('menu-root'),
    ContextMenuTrigger: plain('menu-trigger'),
    ContextMenuPortal: plain('menu-portal'),
    ContextMenuContent: plain('menu-content'),
    ContextMenuItem: plain('menu-item'),
    ContextMenuLabel: plain('menu-label'),
    ContextMenuSeparator: plain('menu-separator'),
  }
})

const { default: MatchRowContextMenu } = await import('./MatchRowContextMenu.vue')

function items(root: FakeNode): FakeNode[] {
  return findAll(root, (node) => node.tag === 'menu-item')
}

function itemTexts(root: FakeNode): string[] {
  return items(root).map((node) => textOf(node).trim())
}

describe('the match row menu (issue #332)', () => {
  it('offers "Set deck..." on a list of constructed matches', () => {
    const chosen: string[] = []
    const mounted = mountFake(MatchRowContextMenu, {
      notes: null,
      archetypes: true,
      deck: true,
      onSetDeck: () => chosen.push('set-deck'),
    })
    expect(mounted.warnings).toEqual([])
    expect(itemTexts(mounted.root)).toContain('Set deck...')
    const labels = findAll(mounted.root, (node) => node.tag === 'menu-label').map((node) => textOf(node).trim())
    expect(labels).toContain('Deck')

    const setDeck = items(mounted.root).find((node) => textOf(node).trim() === 'Set deck...')
    ;(setDeck?.props.onSelect as () => void)()
    expect(chosen).toEqual(['set-deck'])
  })

  it('leaves it out where a match has no deck to choose (a Limited list)', () => {
    const mounted = mountFake(MatchRowContextMenu, { notes: null })
    expect(itemTexts(mounted.root)).not.toContain('Set deck...')
  })

  it('offers "Edit match..." first on a stored match (issue #393)', () => {
    const chosen: string[] = []
    const mounted = mountFake(MatchRowContextMenu, { notes: null, editable: true, onEdit: () => chosen.push('edit') })
    expect(itemTexts(mounted.root)[0]).toBe('Edit match...')
    ;(items(mounted.root)[0]?.props.onSelect as () => void)()
    expect(chosen).toEqual(['edit'])

    const unstored = mountFake(MatchRowContextMenu, { notes: null })
    expect(itemTexts(unstored.root)).not.toContain('Edit match...')
  })

  it('names the hide "Delete match"', () => {
    const removed: string[] = []
    const mounted = mountFake(MatchRowContextMenu, { notes: null, onRemove: () => removed.push('x') })
    expect(itemTexts(mounted.root)).toContain('Delete match')
    expect(itemTexts(mounted.root)).not.toContain('Remove from stats')
    const remove = items(mounted.root).find((node) => textOf(node).trim() === 'Delete match')
    ;(remove?.props.onSelect as () => void)()
    expect(removed).toEqual(['x'])
  })

  it('offers "Move to league..." for a finished match only', () => {
    const chosen: string[] = []
    const movable = mountFake(MatchRowContextMenu, { notes: null, movable: true, onMoveToLeague: () => chosen.push('move') })
    const move = items(movable.root).find((node) => textOf(node).trim() === 'Move to league...')
    ;(move?.props.onSelect as () => void)()
    expect(chosen).toEqual(['move'])
    const fixed = mountFake(MatchRowContextMenu, { notes: null })
    expect(itemTexts(fixed.root)).not.toContain('Move to league...')
  })
})
