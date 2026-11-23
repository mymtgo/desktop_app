import { describe, expect, it, vi } from 'vitest'
import type { ReplayShareState } from '@/bindings'
import { findAll, mountFake, textOf, type FakeNode } from '../replayTestHarness'

vi.mock('@/bindings', () => ({ commands: {} }))

const { default: ReplayShareBody } = await import('./ReplayShareBody.vue')

const unshared: ReplayShareState = { linked: true, supporter: true, shareable: true, url: null }
const shared: ReplayShareState = { linked: true, supporter: true, shareable: true, url: 'https://mymtgo.test/players/Qplayer/replays/u?game=2' }

function body(share: ReplayShareState | null, extra: Record<string, unknown> = {}) {
  const mounted = mountFake(ReplayShareBody, { share, busy: false, error: null, copied: false, ...extra })
  const buttons = findAll(mounted.root, (node) => node.tag === 'button')
  const button = (label: string): FakeNode | undefined => buttons.find((node) => textOf(node).trim() === label)
  return { mounted, text: textOf(mounted.root), button, buttons }
}

describe('the share dialog body', () => {
  it('asks to sign in on the Account page and has no main button when not signed in', () => {
    const { text, button } = body({ linked: false, supporter: false, shareable: true, url: null })
    expect(text).toContain('Sign in on the Account page to share replays.')
    expect(button('Create link')).toBeUndefined()
    expect(button('Update link')).toBeUndefined()
  })

  it('tells a free, unshared, signed in player sharing is a supporter feature', () => {
    const { text, button } = body({ linked: true, supporter: false, shareable: true, url: null })
    expect(text).toContain('Sharing replays is a supporter feature.')
    expect(button('Create link')).toBeDefined()
  })

  it('shows no supporter note for a supporter', () => {
    expect(body(unshared).text).not.toContain('supporter feature')
  })

  it('shows the link, copy, the update note, switch off and update when shared', () => {
    const { mounted, text, button } = body({ ...shared, supporter: false })
    const input = findAll(mounted.root, (node) => node.tag === 'input')[0]
    expect(input.props.value).toBe(shared.url)
    expect(input.props['aria-label']).toBe('Replay link')
    expect(button('Copy link')).toBeDefined()
    expect(text).toContain('Played more games since sharing? Update the link to add them.')
    expect(button('Switch off link')).toBeDefined()
    expect(button('Update link')).toBeDefined()
    expect(text).not.toContain('supporter feature')
  })

  it('shows Uploading... and disables both buttons while busy', () => {
    const { button } = body(shared, { busy: true })
    const main = button('Uploading...')
    expect(main).toBeDefined()
    expect(main!.props.disabled).toBeTruthy()
    expect(button('Switch off link')!.props.disabled).toBeTruthy()
  })

  it('shows the error line', () => {
    expect(body(unshared, { error: 'This game is too large to share.' }).text).toContain('This game is too large to share.')
  })

  it('says Copied after copying', () => {
    const { button } = body(shared, { copied: true })
    expect(button('Copied')).toBeDefined()
    expect(button('Copy link')).toBeUndefined()
  })

  it('selects the whole link when the field is focused (0.x)', () => {
    const { mounted } = body(shared)
    const input = findAll(mounted.root, (node) => node.tag === 'input')[0]
    const select = vi.fn()
    ;(input.props.onFocus as (event: unknown) => void)({ target: { select } })
    expect(select).toHaveBeenCalledTimes(1)
    expect(String(input.props.class)).toContain('font-mono')
    expect(String(input.props.class)).toContain('text-xs')
  })

  it('draws a Copy icon, and a Check once copied (0.x)', () => {
    const icons = (copied: boolean) => {
      const { button } = body(shared, { copied })
      return findAll(button(copied ? 'Copied' : 'Copy link')!, (node) => node.tag === 'svg').map((node) =>
        String(node.props.class),
      )
    }
    expect(icons(false)[0]).toContain('lucide-copy')
    expect(icons(true)[0]).toContain('lucide-check')
  })

  it('keeps its buttons in a footer', () => {
    const { mounted } = body(shared)
    const footer = findAll(mounted.root, (node) => String(node.props['data-slot']) === 'dialog-footer')
    expect(footer).toHaveLength(1)
    expect(findAll(footer[0], (node) => node.tag === 'button').map((node) => textOf(node).trim())).toEqual([
      'Switch off link',
      'Update link',
    ])
  })

  it('emits share, revoke and copy from its buttons', () => {
    const onShare = vi.fn()
    const onRevoke = vi.fn()
    const onCopy = vi.fn()
    const { button } = body(shared, { onShare, onRevoke, onCopy })
    ;(button('Update link')!.props.onClick as () => void)()
    ;(button('Switch off link')!.props.onClick as () => void)()
    ;(button('Copy link')!.props.onClick as () => void)()
    expect(onShare).toHaveBeenCalledTimes(1)
    expect(onRevoke).toHaveBeenCalledTimes(1)
    expect(onCopy).toHaveBeenCalledTimes(1)
  })
})
