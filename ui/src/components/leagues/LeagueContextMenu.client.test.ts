import { describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

// The dropdown as plain elements: the fake renderer has no portal. An item
// keeps its `onSelect` listener and `disabled` as props.
vi.mock('@/components/ui/dropdown-menu', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    DropdownMenu: plain('menu'),
    DropdownMenuTrigger: plain('menu-trigger'),
    DropdownMenuContent: plain('menu-content'),
    DropdownMenuItem: plain('menu-item'),
    DropdownMenuSeparator: plain('menu-separator'),
    DropdownMenuSub: plain('menu-sub'),
    DropdownMenuSubTrigger: plain('menu-sub-trigger'),
    DropdownMenuSubContent: plain('menu-sub-content'),
  }
})

const { default: LeagueContextMenu } = await import('./LeagueContextMenu.vue')

function item(root: FakeNode, text: string): FakeNode {
  const found = findAll(root, (node) => node.tag === 'menu-item').find((node) => textOf(node).includes(text))
  if (!found) throw new Error(`no item ${text}`)
  return found
}

const BASE = { canDelete: true, canAddMatches: true, userState: null, autoState: 'complete' }

describe('the league "..." menu', () => {
  it('lists the items in the spec order', () => {
    const mounted = mountFake(LeagueContextMenu, BASE)
    expect(mounted.warnings).toEqual([])
    const texts = findAll(mounted.root, (node) => node.tag === 'menu-item' || node.tag === 'menu-sub-trigger').map(
      (node) => textOf(node).trim(),
    )
    expect(texts.slice(0, 4)).toEqual([
      'Copy screenshot of league',
      'Copy league summary',
      'Edit notes',
      'Add matches...',
    ])
    expect(texts).toContain('Set status')
    expect(texts.at(-1)).toContain('Delete league...')
    expect(texts.join('|')).not.toContain('Drop league')
  })

  it('leaves the copy items out when not shareable', () => {
    const mounted = mountFake(LeagueContextMenu, { ...BASE, shareable: false })
    const texts = findAll(mounted.root, (node) => node.tag === 'menu-item').map((node) => textOf(node).trim())
    expect(texts.join('|')).not.toContain('Copy')
    expect(texts[0]).toBe('Edit notes')
  })

  it('offers only Set status when status only (an Arena run)', () => {
    const mounted = mountFake(LeagueContextMenu, { ...BASE, statusOnly: true })
    const top = findAll(mounted.root, (node) => node.tag === 'menu-item' || node.tag === 'menu-sub-trigger')
      .map((node) => textOf(node).trim())
      .join('|')
    expect(top).toContain('Set status')
    for (const hidden of ['Copy', 'Edit notes', 'Add matches...', 'Delete league...']) {
      expect(top).not.toContain(hidden)
    }
    expect(item(mounted.root, 'Automatic (Complete)').props['data-checked']).toBe(true)
  })

  it('marks Automatic with the fold state, and emits each status', () => {
    const chosen: (string | null)[] = []
    const mounted = mountFake(LeagueContextMenu, { ...BASE, onSetStatus: (value: string | null) => chosen.push(value) })
    const automatic = item(mounted.root, 'Automatic (Complete)')
    expect(automatic.props['data-checked']).toBe(true)
    expect(item(mounted.root, 'Partial').props['data-checked']).toBe(false)
    ;(automatic.props.onSelect as () => void)()
    ;(item(mounted.root, 'Dropped').props.onSelect as () => void)()
    expect(chosen).toEqual([null, 'dropped'])
  })

  it('checks the player status when one is set', () => {
    const mounted = mountFake(LeagueContextMenu, { ...BASE, userState: 'partial' })
    expect(item(mounted.root, 'Partial').props['data-checked']).toBe(true)
    expect(item(mounted.root, 'Automatic (Complete)').props['data-checked']).toBe(false)
  })

  it('disables Delete league while Active, with the hint', () => {
    const deleted: string[] = []
    const mounted = mountFake(LeagueContextMenu, { ...BASE, canDelete: false, onDelete: () => deleted.push('x') })
    const remove = item(mounted.root, 'Delete league...')
    expect(remove.props.disabled).toBe(true)
    expect(textOf(remove)).toContain('Drop it first')
  })
})
