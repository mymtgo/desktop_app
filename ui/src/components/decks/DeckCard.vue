<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { computed } from 'vue'
import type { DeckView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import DeckBadges from '@/components/decks/DeckBadges.vue'
import DeckSelectHandle from '@/components/decks/DeckSelectHandle.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import WinRateBar from '@/components/decks/WinRateBar.vue'
import { Card } from '@/components/ui/card'
import { Badge } from '@/components/ui/badge'
import { activityText, deckCover, deckKindLabels, manaWash } from '@/lib/decks'

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

/**
 * Decks without cover art still fill the art band, tinted by their colour
 * identity, so the grid has no holes where a deck has no art yet.
 */
const fallbackArtStyle = computed(() => ({ backgroundImage: manaWash(props.deck.colors) }))
const activity = computed(() => activityText(props.deck, props.now, 'never played'))
const record = computed(() => props.deck.record)
const cover = computed(() => deckCover(props.deck))
const kinds = computed(() => deckKindLabels(props.deck))
</script>

<template>
  <RouterLink :to="`/decks/${deck.id}`" class="group block h-full cursor-pointer outline-none">
    <Card
      class="relative flex h-full flex-col gap-0 overflow-hidden py-0 transition-colors group-hover:border-white/10 group-hover:bg-black/25 group-focus-visible:border-ring"
      :class="[selected && 'border-primary/60', dimmed && 'opacity-40']"
    >
      <CardImage
        v-if="cover.hasCover"
        :src="cover.src"
        :local="cover.local"
        :custom="cover.custom"
        :alt="deck.name"
        class="pointer-events-none absolute inset-0 h-full w-full object-cover object-top transition-transform duration-500 ease-out group-hover:scale-[1.04]"
        :class="deck.archived ? 'grayscale' : ''"
      />
      <div v-else class="pointer-events-none absolute inset-0" :class="deck.archived ? 'grayscale' : ''" :style="fallbackArtStyle" />

      <!-- A gradient scrim, dark enough at the foot of the card to carry the stats text. -->
      <div class="pointer-events-none absolute inset-0 bg-linear-to-t from-black via-black/75 to-black/20" />

      <!-- The bulk assign's checkbox; a click on it never opens the deck. -->
      <div
        v-if="selectable"
        class="absolute top-3 left-3 z-10 flex items-center gap-2 rounded-md border border-white/10 bg-neutral-900/90 p-1.5 shadow shadow-black/50"
        @click.stop.prevent
      >
        <DeckSelectHandle
          :deck="deck"
          :selected="selected"
          :selection-blocked="selectionBlocked"
          :selected-ids="selectedIds"
          checkbox-class="size-4.5 border-white/60 bg-neutral-800"
          @update:selected="(id: number, value: boolean) => emit('update:selected', id, value)"
          @dragstart="(format: string) => emit('dragstart', format)"
          @dragend="emit('dragend')"
        />
      </div>

      <!-- Pinned to the card so a long name gets the full tile width. -->
      <ManaColors
        v-if="deck.colors.length > 0"
        :colors="deck.colors"
        class="absolute top-3 right-3 z-10 rounded-full border border-white/10 bg-black/60 px-2 py-1 shadow shadow-black/50 backdrop-blur-sm"
      />

      <!-- Spacer that gives the art band room to read as art. -->
      <div class="relative h-44 shrink-0" />

      <div class="relative flex flex-1 flex-col justify-end gap-3 p-4 [text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]">
        <span class="line-clamp-2 text-base leading-tight font-semibold">{{ deck.name }}</span>

        <div class="flex min-w-0 items-center gap-2 text-xs">
          <DeckBadges :deck="deck" />
          <Badge v-for="kind in kinds" :key="kind" variant="secondary" class="shrink-0 py-0 text-xs" data-testid="deck-kind">{{ kind }}</Badge>
        </div>

        <div class="flex flex-col gap-1.5">
          <WinRateBar :winrate="record.winRate" solid />
          <div class="flex items-baseline justify-between gap-2 text-xs tabular-nums">
            <span class="truncate text-muted-foreground">{{ activity }}</span>
            <MatchRecord :record="record" />
          </div>
        </div>
      </div>
    </Card>
  </RouterLink>
</template>
