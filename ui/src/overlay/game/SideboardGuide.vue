<script setup lang="ts">
import { computed } from 'vue'
import type { OverlayNotes, SideboardGuide, SidedInCard, SidedOutCard } from '@/bindings'
import { useCardHoverPreview } from '@/composables/useCardHoverPreview'
import { groupByType } from '@/composables/useCardTypeGroups'
import {
  cutCell,
  sideboardEmpty,
  sideboardHeader,
  sidedInFaded,
  sidedInName,
  sidedOutHeading,
  sidedOutName,
  winLossCell,
} from '@/lib/overlay'
import ArchetypeNotes from './ArchetypeNotes.vue'
import CardPreview from './CardPreview.vue'
import OverlayCardRow from './OverlayCardRow.vue'

/**
 * The sideboard guide pane (0.x `SideboardGuide`): the player's plan for
 * the matchup when there is one, else their history (cards brought in by
 * type, cards usually cut), with the "In" and "Out" badges from their own
 * games, then the matchup notes. No community "Field" column (spec ruling
 * 6).
 */
const props = defineProps<{
  sideboard: SideboardGuide | null
  notes: OverlayNotes
  hasMatch: boolean
  hasDeck: boolean
  hasArchetype: boolean
}>()

const empty = computed(() =>
  sideboardEmpty({ hasMatch: props.hasMatch, hasDeck: props.hasDeck, hasArchetype: props.hasArchetype }),
)
const hasPlan = computed(() => props.sideboard?.hasPlan ?? false)
const groupedSidedIn = computed<Record<string, SidedInCard[]>>(() =>
  groupByType(props.sideboard?.sidedIn ?? [], (card) => card.cardType),
)
const sidedOut = computed<SidedOutCard[]>(() => props.sideboard?.sidedOut ?? [])
const groupQuantity = (cards: SidedInCard[]): number => cards.reduce((sum, card) => sum + card.quantity, 0)

const { hoveredCard, previewTop, onCardEnter, onCardLeave } = useCardHoverPreview<SidedInCard | SidedOutCard>()
</script>

<template>
  <div class="relative flex flex-col">
    <p v-if="empty" class="p-6 text-center text-xs text-muted-foreground">{{ empty }}</p>

    <template v-else-if="props.sideboard">
      <div class="px-4 py-2 text-[0.625rem] font-semibold tracking-wider text-muted-foreground/60 uppercase">
        {{ sideboardHeader(props.sideboard) }}
      </div>

      <div v-for="(cards, type) in groupedSidedIn" :key="type">
        <h3
          class="flex items-baseline justify-between gap-2 border-y py-2 pl-4 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase"
        >
          <span class="min-w-0 truncate">{{ type }} ({{ groupQuantity(cards) }})</span>
          <span class="flex shrink-0 items-center gap-2 px-2">
            <span v-if="!hasPlan" class="w-8" aria-hidden="true"></span>
            <span class="w-20 text-right">Your W–L</span>
          </span>
        </h3>
        <div class="divide-y text-xs">
          <OverlayCardRow
            v-for="card in cards"
            :key="card.oracleId"
            :name="sidedInName(card)"
            :count="card.quantity"
            :art-crop="card.artCrop"
            :local-art-crop="card.localArtCrop"
            :class="{ 'opacity-40': sidedInFaded(card, hasPlan) }"
            @preview-enter="onCardEnter(card, $event)"
            @preview-leave="onCardLeave"
          >
            <div class="flex shrink-0 items-center gap-2 px-2">
              <span v-if="!hasPlan" class="w-8 text-right text-[10px] font-bold tracking-wider uppercase">
                <span v-if="card.recommended" class="text-emerald-400">In</span>
              </span>
              <span class="w-20 text-right font-medium text-muted-foreground tabular-nums">{{ winLossCell(card) }}</span>
            </div>
          </OverlayCardRow>
        </div>
      </div>

      <template v-if="sidedOut.length">
        <h3
          class="flex items-baseline justify-between gap-2 border-y py-2 pl-4 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase"
        >
          <span class="min-w-0 truncate">{{ sidedOutHeading(props.sideboard) }}</span>
          <span class="flex shrink-0 items-center gap-2 px-2">
            <span v-if="!hasPlan" class="w-8" aria-hidden="true"></span>
            <span class="w-20 text-right">You cut</span>
          </span>
        </h3>
        <div class="divide-y text-xs">
          <OverlayCardRow
            v-for="card in sidedOut"
            :key="card.oracleId"
            :name="sidedOutName(card)"
            :count="hasPlan ? card.quantity : null"
            :art-crop="card.artCrop"
            :local-art-crop="card.localArtCrop"
            :class="{ 'opacity-40': card.stale }"
            @preview-enter="onCardEnter(card, $event)"
            @preview-leave="onCardLeave"
          >
            <div class="flex shrink-0 items-center gap-2 px-2">
              <span v-if="!hasPlan" class="w-8 text-right text-[10px] font-bold tracking-wider uppercase">
                <span v-if="card.recommended" class="text-red-400">Out</span>
              </span>
              <span class="w-20 text-right font-medium text-muted-foreground tabular-nums">{{ cutCell(card) }}</span>
            </div>
          </OverlayCardRow>
        </div>
      </template>
    </template>

    <div class="px-3 pt-3 pb-3">
      <ArchetypeNotes :current="props.notes.current" :other="props.notes.other" :disabled="!props.hasDeck || !props.hasArchetype" />
    </div>

    <CardPreview :card="hoveredCard" :top="previewTop" />
  </div>
</template>
