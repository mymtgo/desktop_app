<script setup lang="ts">
import { ChevronLeft, ChevronRight, X } from 'lucide-vue-next'
import { computed, onMounted, ref } from 'vue'
import type { WrappedCard } from '@/bindings'
import CopyImageButton from '@/components/metagame/CopyImageButton.vue'
import { trackUsage } from '@/lib/usage'

/**
 * Wrapped's story (Wrapped spec, mockup section 2): one card centred, back
 * and forward buttons, ← and → keys, Home to start again, Escape to close.
 * Cards cross-fade. The slot renders each card and gets `restart` for the
 * closing card's "Watch again". "Copy as image" puts the card on show on the
 * clipboard, for sharing as a screenshot, with a small MyMTGO watermark
 * that shows only in the image, and counts which kind of card was copied
 * (the kind only, usage analytics spec).
 */
const props = defineProps<{ cards: WrappedCard[]; year: number }>()
const emit = defineEmits<{ close: [] }>()

const index = ref(0)
const stage = ref<HTMLElement | null>(null)
// The card on show, for "Copy as image".
const shown = ref<HTMLElement | null>(null)

function setShown(element: unknown) {
  shown.value = (element as HTMLElement | null) ?? null
}

const current = computed(() => props.cards[Math.min(index.value, props.cards.length - 1)])
const atStart = computed(() => index.value === 0)
const atEnd = computed(() => index.value >= props.cards.length - 1)

function go(next: number) {
  index.value = Math.max(0, Math.min(next, props.cards.length - 1))
  // A clicked button can vanish (Watch again) or turn disabled (first or
  // last card), which drops focus to the body and the keys with it.
  stage.value?.focus?.()
}

function copied() {
  trackUsage({ kind: 'wrappedCardCopied', card: current.value.kind })
}

function restart() {
  go(0)
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'ArrowRight') go(index.value + 1)
  else if (event.key === 'ArrowLeft') go(index.value - 1)
  else if (event.key === 'Home') restart()
  else if (event.key === 'Escape') emit('close')
  else return
  event.preventDefault()
}

// Focus the stage so the keys work without a click first.
onMounted(() => stage.value?.focus?.())
</script>

<template>
  <div
    ref="stage"
    class="wrapped-stage relative flex min-h-[calc(100vh-7rem)] flex-col items-center justify-center gap-6 overflow-hidden px-16 py-12 outline-none"
    tabindex="0"
    @keydown="onKeydown"
  >
    <div class="absolute inset-x-6 top-4 flex items-center justify-between text-xs text-muted-foreground">
      <span><span class="font-semibold text-foreground">MyMTGO Wrapped</span> · {{ year }}</span>
      <span class="flex items-center gap-3">
        <span class="wrapped-counter tabular-nums">{{ index + 1 }} / {{ cards.length }}</span>
        <button type="button" class="flex items-center gap-1 hover:text-foreground" aria-label="Close Wrapped" @click="emit('close')">
          Close <X class="size-3.5" />
        </button>
      </span>
    </div>

    <div class="relative flex w-full max-w-4xl items-center gap-6">
      <button
        type="button"
        class="wrapped-arrow"
        aria-label="Previous card"
        :disabled="atStart"
        @click="go(index - 1)"
      >
        <ChevronLeft class="size-5" />
      </button>

      <!-- At least 16:9; a card with more to show grows taller. The spacer
           and the card share one grid cell, so the row is the taller of the two. -->
      <div class="relative min-w-0 flex-1">
        <Transition name="wrapped-fade" mode="out-in">
          <div :key="index" class="grid grid-cols-[minmax(0,1fr)]">
            <div class="col-start-1 row-start-1 aspect-video" aria-hidden="true" />
            <div :ref="setShown" class="wrapped-shown relative col-start-1 row-start-1 min-w-0">
              <slot name="card" :card="current" :index="index" :restart="restart" />
              <div class="wrapped-watermark" aria-hidden="true">
                <span class="font-semibold text-white/80">MyMTGO Wrapped {{ year }}</span>
                <span>mymtgo.com</span>
              </div>
            </div>
          </div>
        </Transition>
      </div>

      <button type="button" class="wrapped-arrow" aria-label="Next card" :disabled="atEnd" @click="go(index + 1)">
        <ChevronRight class="size-5" />
      </button>
    </div>

    <div class="flex flex-col items-center gap-4">
      <div class="flex items-center gap-1.5">
        <button
          v-for="(_, dot) in cards"
          :key="dot"
          type="button"
          class="wrapped-dot h-1.5 rounded-full transition-all"
          :class="dot === index ? 'w-5 bg-foreground' : 'w-1.5 bg-muted-foreground/40 hover:bg-muted-foreground'"
          :aria-label="`Card ${dot + 1}`"
          @click="go(dot)"
        />
      </div>
      <CopyImageButton :target="shown" :file-name="`mymtgo-wrapped-${year}-${index + 1}.png`" @copied="copied" />
    </div>
  </div>
</template>

<style scoped>
/* Only in the copied image: useScreenshot marks the target while it renders. */
.wrapped-watermark {
  display: none;
  position: absolute;
  right: 1rem;
  bottom: 0.75rem;
  align-items: center;
  gap: 0.5rem;
  border-radius: 9999px;
  background: rgb(0 0 0 / 55%);
  padding: 0.25rem 0.75rem;
  font-size: 0.7rem;
  color: rgb(255 255 255 / 60%);
  pointer-events: none;
}
[data-capturing] > .wrapped-watermark {
  display: flex;
}
.wrapped-arrow {
  display: flex;
  flex: none;
  align-items: center;
  justify-content: center;
  width: 2.5rem;
  height: 2.5rem;
  border-radius: 9999px;
  border: 1px solid var(--color-border);
  background: var(--color-card);
  color: var(--color-foreground);
}
.wrapped-arrow:hover:not(:disabled) {
  background: var(--color-accent);
}
.wrapped-arrow:disabled {
  opacity: 0.3;
}
.wrapped-fade-enter-active,
.wrapped-fade-leave-active {
  transition:
    opacity 250ms ease,
    transform 250ms ease;
}
.wrapped-fade-enter-from {
  opacity: 0;
  transform: translateY(6px);
}
.wrapped-fade-leave-to {
  opacity: 0;
}
@media (prefers-reduced-motion: reduce) {
  .wrapped-fade-enter-active,
  .wrapped-fade-leave-active {
    transition: none;
  }
}
</style>
