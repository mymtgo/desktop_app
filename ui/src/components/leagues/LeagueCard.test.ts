import { parse } from 'vue/compiler-sfc'
import { describe, expect, it } from 'vitest'
import cardSource from './LeagueCard.vue?raw'

// Element nodes of the compiled template tree (type 1), with their tag.
type Node = { type: number; tag?: string; props?: { name: string; value?: { content: string } }[]; children?: Node[] }

// Native controls, the app's button and link components, and the wrappers that
// render a control (menu, dialog and popover triggers, toggles, fields).
const INTERACTIVE = new Set([
  'button',
  'a',
  'input',
  'select',
  'textarea',
  'Button',
  'RouterLink',
  'LeagueContextMenu',
  'DropdownMenuTrigger',
  'DialogTrigger',
  'PopoverTrigger',
  'Toggle',
  'Switch',
  'Checkbox',
  'Input',
  'Textarea',
])

function source(): Node {
  const { descriptor } = parse(cardSource)
  return descriptor.template?.ast as unknown as Node
}

/** Interactive elements that sit inside another interactive element. */
function nested(node: Node, insideInteractive = false, found: string[] = []): string[] {
  const interactive = node.type === 1 && INTERACTIVE.has(node.tag ?? '')
  if (interactive && insideInteractive) found.push(node.tag ?? '')
  for (const child of node.children ?? []) nested(child, insideInteractive || interactive, found)
  return found
}

function elements(node: Node, out: Node[] = []): Node[] {
  if (node.type === 1) out.push(node)
  for (const child of node.children ?? []) elements(child, out)
  return out
}

describe('LeagueCard (#143, a11y)', () => {
  it('has no interactive element inside another', () => {
    expect(nested(source())).toEqual([])
  })

  it('opens and closes through one named button that reports its state', () => {
    const toggles = elements(source()).filter(
      (node) => node.tag === 'button' && JSON.stringify(node.props).includes('aria-expanded'),
    )
    expect(toggles).toHaveLength(1)
    // The name follows the state: "Hide" while open, "Show" while closed.
    const props = JSON.stringify(toggles[0].props)
    expect(props).toContain('aria-label')
    expect(props).toContain('Hide league details')
    expect(props).toContain('Show league details')
  })
})
