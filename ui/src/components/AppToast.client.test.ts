import { describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

vi.stubGlobal('requestAnimationFrame', (callback: (time: number) => void) => {
  callback(0)
  return 0
})

const { default: AppToast } = await import('./AppToast.vue')

describe('a toast with an action', () => {
  it('shows the action, runs it once and dismisses the toast', () => {
    const ran: string[] = []
    const dismissed: number[] = []
    const mounted = mountFake(AppToast, {
      toast: {
        id: 4,
        type: 'success',
        title: 'League deleted',
        message: 'You can restore it from Settings, Archives.',
        duration: 10_000,
        action: { label: 'Undo', run: () => ran.push('undo') },
      },
      onDismiss: (id: number) => dismissed.push(id),
    })
    expect(mounted.warnings).toEqual([])
    const [button] = findAll(mounted.root, (node) => node.props['data-testid'] === 'toast-action')
    expect(textOf(button)).toBe('Undo')
    ;(button.props.onClick as (event: { stopPropagation: () => void }) => void)({ stopPropagation: () => {} })
    expect(ran).toEqual(['undo'])
    expect(dismissed).toEqual([4])
  })

  it('has no action button without an action', () => {
    const mounted = mountFake(AppToast, {
      toast: { id: 1, type: 'error', title: 'Could not delete', message: '', duration: 5000 },
    })
    expect(findAll(mounted.root, (node) => node.props['data-testid'] === 'toast-action')).toEqual([])
  })
})
