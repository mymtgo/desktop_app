import { describe, expect, it } from 'vitest'
import { findAll, hasClass, mountFake } from '@/replay/replayTestHarness'

const { default: MeterBar } = await import('./MeterBar.vue')

function fill(props: Record<string, unknown>) {
  const mounted = mountFake(MeterBar, props)
  expect(mounted.warnings).toEqual([])
  return findAll(mounted.root, (node) => hasClass(node, 'meter-fill'))[0]
}

function width(props: Record<string, unknown>): string {
  return (fill(props).props.style as { width: string }).width
}

describe('the meter bar', () => {
  it('clamps its width from 0 to 100', () => {
    expect(width({ value: 43 })).toBe('43%')
    expect(width({ value: 250 })).toBe('100%')
    expect(width({ value: -5 })).toBe('0%')
  })

  it('is green by default and takes a fill class', () => {
    expect(hasClass(fill({ value: 10 }), 'bg-success')).toBe(true)
    const custom = fill({ value: 10, fillClass: 'bg-yellow-400' })
    expect(hasClass(custom, 'bg-yellow-400')).toBe(true)
    expect(hasClass(custom, 'bg-success')).toBe(false)
  })
})
