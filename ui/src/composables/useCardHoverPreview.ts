import { ref } from 'vue'

/** The preview image's height the window keeps room for (0.x: about 280 px). */
export const PREVIEW_HEIGHT = 280
/** The preview's distance from the window's top edge, at least. */
export const PREVIEW_MARGIN = 8

type PreviewableCard = { name: string; image: string | null; localImage?: string | null }

/** Where the preview's top goes for a row at `rowTop` (clamped inside the window). */
export function previewTopFor(rowTop: number, windowHeight: number): number {
  return Math.max(PREVIEW_MARGIN, Math.min(rowTop, windowHeight - PREVIEW_HEIGHT))
}

/**
 * The hover card image of the overlay's rows (0.x `useCardHoverPreview`):
 * 200 px wide, anchored to the window's right edge, beside the hovered
 * thumbnail and clamped so it never leaves the frameless window.
 */
export function useCardHoverPreview<T extends PreviewableCard>() {
  const hoveredCard = ref<T | null>(null)
  const previewTop = ref(0)

  function onCardEnter(card: T, event: MouseEvent): void {
    if (!card.image && !card.localImage) return
    hoveredCard.value = card
    const rowTop = (event.currentTarget as HTMLElement).getBoundingClientRect().top
    previewTop.value = previewTopFor(rowTop, window.innerHeight)
  }

  function onCardLeave(): void {
    hoveredCard.value = null
  }

  return { hoveredCard, previewTop, onCardEnter, onCardLeave }
}
