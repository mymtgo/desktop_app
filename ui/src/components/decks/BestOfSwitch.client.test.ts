import { describe, expect, it } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const { default: BestOfSwitch } = await import('./BestOfSwitch.vue')

describe('BestOfSwitch', () => {
  it('shows Bo3 then Bo1 with the selected one pressed', () => {
    const mounted = mountFake(BestOfSwitch, { modelValue: 'bo1' })
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons.map((node) => textOf(node).trim())).toEqual(['Bo3', 'Bo1'])
    expect(buttons.map((node) => node.props['aria-pressed'])).toEqual([false, true])
  })

  it('emits the clicked kind', () => {
    const emitted: unknown[] = []
    const mounted = mountFake(BestOfSwitch, { modelValue: 'bo1', 'onUpdate:modelValue': (v: unknown) => emitted.push(v) })
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    ;(buttons[0].props.onClick as () => void)()
    expect(emitted).toEqual(['bo3'])
  })
})
