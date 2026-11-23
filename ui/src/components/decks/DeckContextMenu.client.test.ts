import { describe, expect, it, vi } from 'vitest'
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
  }
})

const { default: DeckContextMenu } = await import('./DeckContextMenu.vue')

function item(root: FakeNode, text: string): FakeNode | undefined {
  return findAll(root, (node) => node.tag === 'menu-item').find((node) => textOf(node).trim() === text)
}

describe('the deck card menu', () => {
  it('sets the archetype by hand', () => {
    const chosen: string[] = []
    const mounted = mountFake(DeckContextMenu, { hasArchetype: false, onSetArchetype: () => chosen.push('set') })
    expect(mounted.warnings).toEqual([])
    const labels = findAll(mounted.root, (node) => node.tag === 'menu-label').map((node) => textOf(node).trim())
    expect(labels).toEqual(['Archetype'])

    ;(item(mounted.root, 'Set manually...')?.props.onSelect as () => void)()
    expect(chosen).toEqual(['set'])
  })

  it('clears the archetype only when the deck has one', () => {
    const chosen: string[] = []
    const unclassified = mountFake(DeckContextMenu, { hasArchetype: false })
    expect(item(unclassified.root, 'Clear')?.props.disabled).toBe(true)

    const classified = mountFake(DeckContextMenu, { hasArchetype: true, onClearArchetype: () => chosen.push('clear') })
    const clear = item(classified.root, 'Clear')
    expect(clear?.props.disabled).toBe(false)
    ;(clear?.props.onSelect as () => void)()
    expect(chosen).toEqual(['clear'])
  })
})
