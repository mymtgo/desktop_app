<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, useTemplateRef, watch } from 'vue'

/**
 * Shows the first of `count` candidates that fits its element (event overlay
 * spec 5.2): the slot renders candidate `index`, and while the element
 * overflows (`scrollWidth > clientWidth`) the next, shorter one is tried.
 * Measured in the rendered slot, so the choice holds in any font; it runs
 * again when the candidates change (`fitKey`), once the fonts have loaded,
 * and when the element or a sibling changes width. The
 * element keeps its ellipsis as the last-resort guard.
 */
const props = withDefaults(defineProps<{ count: number; fitKey: string; tag?: string }>(), { tag: 'span' })

const root = useTemplateRef<HTMLElement>('root')
const index = ref(0)
let run = 0

function overflows(element: HTMLElement | null): boolean {
  // Elements without layout (tests, no DOM) always fit.
  if (!element || typeof element.scrollWidth !== 'number' || typeof element.clientWidth !== 'number') return false
  if (element.scrollWidth > element.clientWidth) return true
  // scrollWidth is a whole number: text a fraction of a pixel too wide still
  // shows an ellipsis, so the text's own width is compared as well.
  const range = element.ownerDocument?.createRange?.()
  if (!range) return false
  range.selectNodeContents(element)
  return range.getBoundingClientRect().width > element.getBoundingClientRect().width + 0.01
}

async function fit(): Promise<void> {
  run += 1
  const mine = run
  index.value = 0
  await nextTick()
  while (mine === run && index.value < props.count - 1 && overflows(root.value)) {
    index.value += 1
    await nextTick()
  }
  if (mine === run && observer) recordWidths()
}

// Re-fits when the element or a sibling changes width (Dots to Letters, a
// wider record, the thumbnail): observing siblings too catches the room
// growing while a short candidate shows. Widths are recorded after each fit,
// so the fit's own candidate swaps never trigger another fit. Older OBS
// browsers without ResizeObserver keep the other triggers.
let observer: ResizeObserver | null = null
const widths = new Map<Element, number>()

function observed(): Element[] {
  const element = root.value
  if (!element) return []
  return element.parentElement ? Array.from(element.parentElement.children) : [element]
}

function recordWidths(): void {
  for (const element of observed()) widths.set(element, Math.round(element.getBoundingClientRect().width))
}

function watchWidths(): void {
  if (typeof globalThis.ResizeObserver !== 'function' || !root.value) return
  observer = new ResizeObserver((entries) => {
    const changed = entries.some((entry) => widths.get(entry.target) !== Math.round(entry.target.getBoundingClientRect().width))
    if (changed) void fit()
  })
  for (const element of observed()) observer.observe(element)
}

onMounted(() => {
  void fit()
  globalThis.document?.fonts?.ready?.then(() => fit())
  watchWidths()
  if (observer) recordWidths()
})

onBeforeUnmount(() => {
  observer?.disconnect()
  observer = null
  widths.clear()
})

watch(
  () => [props.count, props.fitKey],
  () => void fit(),
)
</script>

<template>
  <component :is="tag" ref="root"><slot :index="index" /></component>
</template>
