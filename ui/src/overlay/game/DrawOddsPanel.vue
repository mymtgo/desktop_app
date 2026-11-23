<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import type { DrawOdds, DrawOddsCard } from '@/bindings'
import { useCardHoverPreview } from '@/composables/useCardHoverPreview'
import { groupByType } from '@/composables/useCardTypeGroups'
import {
  FLASH_MS,
  cardKey,
  changedRows,
  clampSample,
  drawChance,
  drawOddsEmpty,
  maxSample,
  nextDrawsLabel,
  pct,
} from '@/lib/overlay'
import CardPreview from './CardPreview.vue'
import OverlayCardRow from './OverlayCardRow.vue'

/**
 * The draw odds pane (0.x `DrawOddsPanel`): the library's cards by type
 * with their copies left, a stepper for the next N draws, the chance of at
 * least one copy per card and per type, and a 1.2 s flash on a row whose
 * count changed.
 */
const props = defineProps<{ drawOdds: DrawOdds | null }>()

const sampleSize = ref(1)
const ceiling = computed(() => maxSample(props.drawOdds))
const canDecrement = computed(() => sampleSize.value > 1)
const canIncrement = computed(() => sampleSize.value < ceiling.value)

function decrement(): void {
  if (canDecrement.value) sampleSize.value--
}

function increment(): void {
  if (canIncrement.value) sampleSize.value++
}

// The library only shrinks during a game: the stepper never passes it.
watch(ceiling, (max) => {
  sampleSize.value = clampSample(sampleSize.value, max)
})

const library = computed(() => props.drawOdds?.librarySize ?? 0)
const cardChance = (card: DrawOddsCard): number => drawChance(library.value, card.remaining, sampleSize.value)
const remainingOf = (cards: DrawOddsCard[]): number => cards.reduce((sum, card) => sum + card.remaining, 0)
const typeChance = (cards: DrawOddsCard[]): number => drawChance(library.value, remainingOf(cards), sampleSize.value)

const grouped = computed<Record<string, DrawOddsCard[]>>(() => groupByType(props.drawOdds?.cards ?? [], (card) => card.cardType))
const empty = computed(() => drawOddsEmpty(props.drawOdds))

const { hoveredCard, previewTop, onCardEnter, onCardLeave } = useCardHoverPreview<DrawOddsCard>()

const previous = new Map<string, number>()
const flashing = ref<Set<string>>(new Set())
const timers: ReturnType<typeof setTimeout>[] = []

watch(
  () => props.drawOdds?.cards,
  (cards) => {
    if (!cards) return
    const next = new Set(flashing.value)
    for (const key of changedRows(previous, cards)) {
      next.add(key)
      timers.push(
        setTimeout(() => {
          const after = new Set(flashing.value)
          after.delete(key)
          flashing.value = after
        }, FLASH_MS),
      )
    }
    flashing.value = next
  },
  { immediate: true },
)

onBeforeUnmount(() => timers.forEach(clearTimeout))
</script>

<template>
  <div class="relative flex h-full flex-col bg-background text-foreground">
    <div v-if="empty" data-tauri-drag-region class="flex h-full items-center justify-center p-6">
      <p class="text-sm text-muted-foreground">{{ empty }}</p>
    </div>

    <div v-else-if="drawOdds" class="flex h-full min-h-0 flex-col">
      <div data-tauri-drag-region class="flex shrink-0 items-center justify-between gap-2 bg-background px-4 py-2">
        <span data-tauri-drag-region class="text-[0.625rem] font-semibold tracking-wider text-muted-foreground/60 uppercase">
          {{ nextDrawsLabel(sampleSize) }}
        </span>
        <div class="flex items-center gap-1">
          <button
            type="button"
            class="flex h-6 w-6 items-center justify-center rounded border border-border text-muted-foreground transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-30"
            :disabled="!canDecrement"
            aria-label="Decrease sample size"
            @click="decrement"
          >
            −
          </button>
          <span class="w-6 text-center text-sm font-semibold tabular-nums">{{ sampleSize }}</span>
          <button
            type="button"
            class="flex h-6 w-6 items-center justify-center rounded border border-border text-muted-foreground transition-colors hover:bg-muted disabled:cursor-not-allowed disabled:opacity-30"
            :disabled="!canIncrement"
            aria-label="Increase sample size"
            @click="increment"
          >
            +
          </button>
        </div>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto pb-4">
        <div v-for="(cards, type) in grouped" :key="type">
          <h3
            class="flex items-baseline justify-between gap-2 border-y py-2 pr-1.5 pl-4 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase"
          >
            <span>{{ type }} ({{ remainingOf(cards) }})</span>
            <span class="w-12 shrink-0 text-right text-muted-foreground tabular-nums">{{ pct(typeChance(cards)) }}</span>
          </h3>
          <div class="divide-y text-xs">
            <OverlayCardRow
              v-for="card in cards"
              :key="cardKey(card)"
              :name="card.name"
              :count="card.remaining"
              :art-crop="card.artCrop"
              :local-art-crop="card.localArtCrop"
              class="transition-colors duration-300"
              :class="{ 'opacity-20': card.remaining === 0, 'is-flashing': flashing.has(cardKey(card)) }"
              @preview-enter="onCardEnter(card, $event)"
              @preview-leave="onCardLeave"
            >
              <div class="flex shrink-0 items-center gap-2 px-2">
                <span class="w-12 text-right font-medium text-muted-foreground tabular-nums">{{ pct(cardChance(card)) }}</span>
              </div>
            </OverlayCardRow>
          </div>
        </div>
      </div>

      <CardPreview :card="hoveredCard" :top="previewTop" />
    </div>
  </div>
</template>

<style scoped>
.is-flashing {
  animation: card-flash 1.2s ease-out;
}

@keyframes card-flash {
  0% {
    background-color: rgba(250, 204, 21, 0.22);
  }
  60% {
    background-color: rgba(250, 204, 21, 0.12);
  }
  100% {
    background-color: transparent;
  }
}
</style>
