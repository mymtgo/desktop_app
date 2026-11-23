import { afterEach, describe, expect, it, vi } from 'vitest'
import { effectScope } from 'vue'
import { PREVIEW_HEIGHT, previewTopFor, useCardHoverPreview } from './useCardHoverPreview'

function hover(top: number): MouseEvent {
  const target = { getBoundingClientRect: () => ({ top }) } as unknown as HTMLElement
  return { currentTarget: target } as unknown as MouseEvent
}

describe('useCardHoverPreview', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('keeps the image inside the window', () => {
    expect(PREVIEW_HEIGHT).toBe(280)
    expect(previewTopFor(100, 600)).toBe(100)
    expect(previewTopFor(500, 600)).toBe(320)
    expect(previewTopFor(-20, 600)).toBe(8)
  })

  it('shows a card with an image and hides it on leave', () => {
    vi.stubGlobal('window', { innerHeight: 300 })
    const scope = effectScope()
    const preview = scope.run(() => useCardHoverPreview<{ name: string; image: string | null }>())!
    preview.onCardEnter({ name: 'Qnone', image: null }, hover(50))
    expect(preview.hoveredCard.value).toBeNull()
    preview.onCardEnter({ name: 'Qbolt', image: 'https://img/q.jpg' }, hover(50))
    expect(preview.hoveredCard.value?.name).toBe('Qbolt')
    expect(preview.previewTop.value).toBe(20)
    preview.onCardLeave()
    expect(preview.hoveredCard.value).toBeNull()
    scope.stop()
  })
})
