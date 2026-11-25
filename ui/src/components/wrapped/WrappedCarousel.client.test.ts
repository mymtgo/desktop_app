import { describe, expect, it } from 'vitest'
import type { WrappedCard } from '@/bindings'
import { findAll, focusLog, hasClass, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const { default: WrappedCarousel } = await import('./WrappedCarousel.vue')

const THREE: WrappedCard[] = [
  { kind: 'yearInNumbers', matches: 34, games: 81, hours: 14, daysPlayed: 9, byDay: [], topPercent: null },
  { kind: 'playerType', label: 'grinder', pills: [] },
  { kind: 'wrap' },
]

function button(root: FakeNode, label: string): FakeNode {
  const found = findAll(root, (node) => node.tag === 'button' && node.props['aria-label'] === label)
  expect(found).toHaveLength(1)
  return found[0]
}

function click(node: FakeNode) {
  ;(node.props.onClick as () => void)()
}

function press(root: FakeNode, key: string) {
  const stage = findAll(root, (node) => hasClass(node, 'wrapped-stage'))[0]
  ;(stage.props.onKeydown as (event: { key: string; preventDefault: () => void }) => void)({ key, preventDefault: () => {} })
}

function counter(root: FakeNode): string {
  return textOf(findAll(root, (node) => hasClass(node, 'wrapped-counter'))[0]).trim()
}

describe('the Wrapped carousel', () => {
  it('starts on the first card with back disabled and forward enabled', () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    expect(mounted.warnings).toEqual([])
    expect(counter(mounted.root)).toBe('1 / 3')
    expect(button(mounted.root, 'Previous card').props.disabled).toBe(true)
    expect(button(mounted.root, 'Next card').props.disabled).toBe(false)
  })

  it('forward and the right arrow key advance; back and the left key go back', async () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    click(button(mounted.root, 'Next card'))
    await mounted.settle()
    expect(counter(mounted.root)).toBe('2 / 3')
    press(mounted.root, 'ArrowRight')
    await mounted.settle()
    expect(counter(mounted.root)).toBe('3 / 3')
    press(mounted.root, 'ArrowLeft')
    await mounted.settle()
    expect(counter(mounted.root)).toBe('2 / 3')
    click(button(mounted.root, 'Previous card'))
    await mounted.settle()
    expect(counter(mounted.root)).toBe('1 / 3')
  })

  it('the last card disables forward, and there is one dot per card', async () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    press(mounted.root, 'ArrowRight')
    press(mounted.root, 'ArrowRight')
    press(mounted.root, 'ArrowRight')
    await mounted.settle()
    expect(counter(mounted.root)).toBe('3 / 3')
    expect(button(mounted.root, 'Next card').props.disabled).toBe(true)
    expect(findAll(mounted.root, (node) => hasClass(node, 'wrapped-dot'))).toHaveLength(3)
  })

  it('offers copy as image, named for the year and the card', () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    expect(textOf(mounted.root)).toContain('Copy as image')
  })

  it('carries a MyMTGO watermark in the card copied, shown only while capturing', () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    const shown = findAll(mounted.root, (node) => hasClass(node, 'wrapped-shown'))[0]
    const watermark = findAll(shown, (node) => hasClass(node, 'wrapped-watermark'))
    expect(watermark).toHaveLength(1)
    expect(textOf(watermark[0])).toContain('MyMTGO Wrapped 2026')
    expect(textOf(watermark[0])).toContain('mymtgo.com')
  })

  it('escape emits close', () => {
    let closed = 0
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE, onClose: () => (closed += 1) })
    press(mounted.root, 'Escape')
    expect(closed).toBe(1)
  })

  it('moving with a button puts focus back on the stage, so the keys keep working', async () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    const stage = findAll(mounted.root, (node) => hasClass(node, 'wrapped-stage'))[0]
    focusLog.length = 0
    click(button(mounted.root, 'Next card'))
    await mounted.settle()
    expect(focusLog.at(-1)).toBe(stage)
  })

  it('restart goes back to the first card', async () => {
    const mounted = mountFake(WrappedCarousel, { year: 2026, cards: THREE })
    press(mounted.root, 'ArrowRight')
    press(mounted.root, 'ArrowRight')
    await mounted.settle()
    press(mounted.root, 'Home')
    await mounted.settle()
    expect(counter(mounted.root)).toBe('1 / 3')
  })
})
