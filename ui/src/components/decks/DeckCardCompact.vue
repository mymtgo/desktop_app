<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { computed } from 'vue'
import type { DeckView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import DeckBadges from '@/components/decks/DeckBadges.vue'
import DeckSelectHandle from '@/components/decks/DeckSelectHandle.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import WinRateBar from '@/components/decks/WinRateBar.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { deckCover, deckKindLabels, lastPlayedText } from '@/lib/decks'

const props = withDefaults(
  defineProps<{
    deck: DeckView
    /** The time "last played" counts back from. */
    now: number
    /** The card shows a checkbox for the bulk archetype assign. */
    selectable?: boolean
    selected?: boolean
    /** The checkbox is off limits (another format's selection, or a Brawl deck). */
    selectionBlocked?: boolean
    /** A selection of another format is active: the card fades back. */
    dimmed?: boolean
    /** Every selected deck: a selected card drags them all. */
    selectedIds?: number[]
  }>(),
  { selectable: false, selected: false, selectionBlocked: false, dimmed: false, selectedIds: () => [] },
)

const emit = defineEmits<{
  'update:selected': [deckId: number, value: boolean]
  /** A drag onto the sidebar started (the deck's format) or ended. */
  dragstart: [format: string]
  dragend: []
}>()

const lastPlayed = computed(() => lastPlayedText(props.deck.lastPlayedAtMs, props.now) ?? 'never')
const record = computed(() => props.deck.record)
const cover = computed(() => deckCover(props.deck))
const kinds = computed(() => deckKindLabels(props.deck))
</script>

<!--
  The dense counterpart to DeckCard: cover art stays a dimmed backdrop behind
  the stats rather than getting a tile of its own, so a card is one text block
  tall. Chosen by the deck list's card size setting.
-->
<template>
  <RouterLink :to="`/decks/${deck.id}`" class="group block cursor-pointer outline-none">
    <Card
      class="relative overflow-hidden transition-colors group-focus-visible:border-ring hover:bg-black/20"
      :class="[selected && 'border-primary/60', dimmed && 'opacity-40']"
    >
      <CardImage
        v-if="cover.hasCover"
        :src="cover.src"
        :local="cover.local"
        :custom="cover.custom"
        :alt="deck.name"
        class="pointer-events-none absolute inset-0 h-full w-full object-cover object-top opacity-50"
        :class="deck.archived ? 'grayscale' : ''"
      />
      <CardContent class="relative flex flex-col gap-3" :class="cover.hasCover ? '[text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]' : ''">
        <div class="flex flex-col gap-1.5">
          <div class="flex min-w-0 items-center gap-2 text-xs">
            <span v-if="selectable" class="flex items-center gap-1.5" @click.stop.prevent>
              <DeckSelectHandle
                :deck="deck"
                :selected="selected"
                :selection-blocked="selectionBlocked"
                :selected-ids="selectedIds"
                checkbox-class="size-4 border-white/60 bg-neutral-800"
                @update:selected="(id: number, value: boolean) => emit('update:selected', id, value)"
                @dragstart="(format: string) => emit('dragstart', format)"
                @dragend="emit('dragend')"
              />
            </span>
            <ManaColors v-if="deck.colors.length > 0" :colors="deck.colors" class="shrink-0" />
            <DeckBadges :deck="deck" />
            <Badge v-for="kind in kinds" :key="kind" variant="secondary" class="shrink-0 py-0 text-xs" data-testid="deck-kind">{{ kind }}</Badge>
          </div>
          <span class="truncate leading-tight font-semibold">{{ deck.name }}</span>
        </div>

        <div class="flex items-end justify-between gap-4">
          <div class="flex flex-1 flex-col gap-1">
            <span class="text-xs text-muted-foreground">win rate</span>
            <WinRateBar :winrate="record.winRate" :solid="cover.hasCover" />
          </div>
          <div class="text-right">
            <div class="text-sm font-medium tabular-nums">
              {{ record.total }} matches
              <span v-if="!deck.archived" class="text-muted-foreground"> · {{ lastPlayed }}</span>
            </div>
            <MatchRecord :record="record" />
          </div>
        </div>
      </CardContent>
    </Card>
  </RouterLink>
</template>
