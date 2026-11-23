<script setup lang="ts">
// One archetype of the Metagame page (spec section 9), built like the deck
// card: the art of one of its cards under a scrim, its name and colours,
// its key cards fanned out (the website's metagame tile), then the player's
// share against the website's, and the deck card's win rate bar and record.
import { computed } from 'vue'
import type { GlobalState, MetagameTileView, RecordView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import WinRateBar from '@/components/decks/WinRateBar.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Card } from '@/components/ui/card'
import { sourceBadge } from '@/lib/archetypes'
import { archetypeLocation, differenceTone, globalText, shareText, tileBackdrop } from '@/lib/metagame'

// The timeframe goes with the link, so the archetype page opens on it (issue #326).
const props = withDefaults(
  defineProps<{ tile: MetagameTileView; globalState: GlobalState; timeframe: string; search?: string; showGlobal?: boolean; month?: string | null; rank?: string | null }>(),
  { showGlobal: true },
)

const backdrop = computed(() => tileBackdrop(props.tile))
const badge = computed(() => sourceBadge(props.tile.source, props.tile.retired, props.tile.offlineHeld))
// The player's share is green above the website's and red below it.
const shareTone = computed(() => differenceTone(props.tile, props.globalState))
const record = computed<RecordView>(() => ({
  wins: props.tile.wins,
  losses: props.tile.losses,
  draws: props.tile.draws,
  total: props.tile.matches,
  winRate: props.tile.winRate,
  label: props.tile.record,
}))
</script>

<template>
  <RouterLink
    :to="archetypeLocation(tile.archetypeId, 'decklist', timeframe, null, search ?? '', { month, rank })"
    class="group block h-full cursor-pointer outline-none"
  >
    <Card
      class="relative flex h-full flex-col gap-0 overflow-hidden py-0 transition-colors group-hover:border-white/10 group-hover:bg-black/25 group-focus-visible:border-ring"
    >
      <CardImage
        v-if="backdrop"
        :src="backdrop.art"
        :local="backdrop.localArt"
        alt=""
        loading="lazy"
        decoding="async"
        class="pointer-events-none absolute inset-0 h-full w-full object-cover object-top transition-transform duration-500 ease-out group-hover:scale-[1.04]"
      />

      <!-- The deck card's scrim, dark enough at the foot to carry the stats. -->
      <div class="pointer-events-none absolute inset-0 bg-linear-to-t from-black via-black/75 to-black/20" />

      <!-- Custom, System or Retired: a tab hanging from the top edge. -->
      <span
        v-if="badge"
        class="absolute top-0 left-1/2 z-10 -translate-x-1/2 rounded-b-md border border-t-0 border-white/10 bg-black/70 px-2 text-[10px] leading-4 font-medium tracking-wider text-muted-foreground uppercase backdrop-blur-sm"
      >
        {{ badge }}
      </span>

      <div class="relative flex min-h-11 min-w-0 items-center gap-2 px-4 pt-3.5">
        <span class="truncate text-base leading-tight font-semibold [text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]" :title="tile.name">{{ tile.name }}</span>
        <ManaColors
          v-if="tile.colors.length > 0"
          :colors="tile.colors"
          class="ml-auto shrink-0 rounded-full border border-white/10 bg-black/60 px-2 py-1 shadow shadow-black/50 backdrop-blur-sm"
        />
      </div>

      <!-- The cards the archetype is built on, overlapping a little like a
           hand set down on the table. Decorative: the name is the label. -->
      <div v-if="tile.cards.length" aria-hidden="true" class="relative mt-3 flex -space-x-3 px-4">
        <CardImage
          v-for="(card, index) in tile.cards"
          :key="card.name"
          :src="card.image"
          :local="card.localImage"
          alt=""
          loading="lazy"
          decoding="async"
          class="relative aspect-[488/680] min-w-0 flex-1 rounded-[4.5%/3.3%] object-cover shadow-[0_10px_24px_-8px_rgba(0,0,0,0.9)] transition-transform duration-300 group-hover:-translate-y-1"
          :style="{ zIndex: String(index) }"
        />
      </div>

      <!-- The shares on the left, the deck card's win rate bar and record on the right. -->
      <div class="relative flex flex-1 items-end gap-6 px-4 pt-3 pb-4 [text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]">
        <dl class="flex shrink-0 gap-6 text-xs">
          <div class="flex flex-col">
            <dt class="text-muted-foreground">Your share</dt>
            <dd
              class="text-base font-semibold tabular-nums"
              :class="{ 'text-success': shareTone === 'up', 'text-destructive': shareTone === 'down' }"
            >
              {{ shareText(tile.share ?? 0) }}
            </dd>
          </div>
          <div v-if="showGlobal" class="flex flex-col">
            <dt class="text-muted-foreground">Global</dt>
            <dd class="text-base font-semibold tabular-nums">{{ globalText(tile, globalState) || '\u00a0' }}</dd>
          </div>
        </dl>

        <div class="flex min-w-0 flex-1 flex-col gap-1.5">
          <WinRateBar :winrate="tile.winRate" solid />
          <div class="flex items-baseline justify-between gap-2 text-xs tabular-nums">
            <span class="truncate text-muted-foreground">{{ tile.matches }} {{ tile.matches === 1 ? 'match' : 'matches' }}</span>
            <MatchRecord :record="record" />
          </div>
        </div>
      </div>
    </Card>
  </RouterLink>
</template>
