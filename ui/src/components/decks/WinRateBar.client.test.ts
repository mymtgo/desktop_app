import { describe, expect, it } from 'vitest'
import { findAll, hasClass, mountFake } from '@/replay/replayTestHarness'

const { default: WinRateBar } = await import('./WinRateBar.vue')

function fillClass(winrate: number): string {
  const mounted = mountFake(WinRateBar, { winrate })
  expect(mounted.warnings).toEqual([])
  const fill = findAll(mounted.root, (node) => hasClass(node, 'meter-fill'))[0]
  return ['bg-destructive', 'bg-muted-foreground', 'bg-success'].filter((name) => hasClass(fill, name)).join()
}

describe('the win rate bar', () => {
  it('colours the fill by the rate: red below 50, grey at 50, green above', () => {
    expect(fillClass(40)).toBe('bg-destructive')
    expect(fillClass(50)).toBe('bg-muted-foreground')
    expect(fillClass(60)).toBe('bg-success')
  })
})
