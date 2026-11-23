<script setup lang="ts">
import { computed } from 'vue'
import type { PotentialCard, PotentialCards, RevealedCard } from '@/bindings'
import { useCardHoverPreview } from '@/composables/useCardHoverPreview'
import { revealsEmpty } from '@/lib/overlay'
import CardPreview from './CardPreview.vue'
import OverlayCardTypeGroups from './OverlayCardTypeGroups.vue'

/**
 * Every card the opponent revealed this match, by type, then (once their
 * archetype is known) what its lists play that has not shown up, with the
 * copies still unaccounted for (0.x `RevealedCards`).
 */
const props = defineProps<{
  reveals: RevealedCard[] | null
  potential: PotentialCards | null
  hasMatch: boolean
}>()

const { hoveredCard, previewTop, onCardEnter, onCardLeave } = useCardHoverPreview<RevealedCard | PotentialCard>()

const potentialGroups = computed(() =>
  [
    { key: 'maindeck', label: 'Could still have', cards: props.potential?.maindeck ?? [] },
    { key: 'sideboard', label: 'Potential sideboard cards', cards: props.potential?.sideboard ?? [] },
  ].filter((group) => group.cards.length > 0),
)

const totalRevealed = computed(() => (props.reveals ?? []).reduce((sum, card) => sum + card.quantity, 0))
const hasReveals = computed(() => (props.reveals ?? []).length > 0)
const empty = computed(() => revealsEmpty({ hasMatch: props.hasMatch, reveals: props.reveals, potential: props.potential }))
</script>

<template>
  <div class="relative flex h-full flex-col bg-background text-foreground">
    <div v-if="empty" data-tauri-drag-region class="flex h-full items-center justify-center p-6">
      <p class="text-sm text-muted-foreground">{{ empty }}</p>
    </div>

    <div v-else class="flex h-full min-h-0 flex-col">
      <div data-tauri-drag-region class="flex shrink-0 items-center justify-between gap-2 bg-background px-4 py-2">
        <span data-tauri-drag-region class="text-[0.625rem] font-semibold tracking-wider text-muted-foreground/60 uppercase">
          Revealed this match
        </span>
        <span class="text-sm font-semibold tabular-nums">{{ totalRevealed }}</span>
      </div>

      <div class="min-h-0 flex-1 overflow-y-auto pb-4">
        <p v-if="!hasReveals" class="border-t px-4 py-2 text-xs text-muted-foreground">Nothing revealed yet</p>

        <OverlayCardTypeGroups :cards="props.reveals ?? []" @enter="onCardEnter" @leave="onCardLeave" />

        <div v-for="group in potentialGroups" :key="group.key" class="mt-3">
          <div class="flex items-center justify-between gap-2 px-4 py-2">
            <span class="text-[0.625rem] font-semibold tracking-wider text-muted-foreground/60 uppercase">{{ group.label }}</span>
          </div>
          <OverlayCardTypeGroups :cards="group.cards" class="text-muted-foreground" @enter="onCardEnter" @leave="onCardLeave" />
        </div>
      </div>

      <CardPreview :card="hoveredCard" :top="previewTop" />
    </div>
  </div>
</template>
