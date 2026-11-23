import { describe, expect, it, vi } from 'vitest'
import type { MetagameBandView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('@/bindings', () => ({ commands: {} }))

const { default: RankTabs } = await import('./RankTabs.vue')

const KEYS = ['bronze', 'silver', 'gold', 'platinum', 'diamond', 'mythic', 'unranked']

function bands(): MetagameBandView[] {
  return KEYS.map((key, index) => ({
    key,
    label: key[0].toUpperCase() + key.slice(1),
    matches: key === 'silver' ? 0 : index + 1,
  }))
}

describe('the Arena rank tabs', () => {
  it('renders All plus one tab per band, named by label', () => {
    const mounted = mountFake(RankTabs, { bands: bands(), modelValue: null })
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons).toHaveLength(8)
    expect(textOf(mounted.root)).toContain('All')
    expect(buttons[3].props['aria-label']).toBe('Gold')
    expect(findAll(mounted.root, (node) => node.tag === 'img')).toHaveLength(7)
  })

  it('disables a band without matches', () => {
    const mounted = mountFake(RankTabs, { bands: bands(), modelValue: null })
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons[2].props.disabled).toBe(true)
    expect(buttons[1].props.disabled).toBeFalsy()
  })

  it('emits the band key, or null for All', () => {
    const emitted: unknown[] = []
    const mounted = mountFake(RankTabs, {
      bands: bands(),
      modelValue: 'gold',
      'onUpdate:modelValue': (value: unknown) => emitted.push(value),
    })
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    ;(buttons[5].props.onClick as () => void)()
    ;(buttons[0].props.onClick as () => void)()
    expect(emitted).toEqual(['diamond', null])
  })

  it('never shows a disabled tab as active', () => {
    const mounted = mountFake(RankTabs, { bands: bands(), modelValue: 'silver' })
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons[2].props['aria-pressed']).toBe(false)
  })
})
