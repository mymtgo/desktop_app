<script setup lang="ts">
import { Hand, PencilLine, Undo2 } from 'lucide-vue-next'
import { computed } from 'vue'
import type { GameDetailView } from '@/bindings'
import CardTile from '@/components/matches/CardTile.vue'
import CardTilePill from '@/components/matches/CardTilePill.vue'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { handGridClass, openingHandTiles, openingHandTitle } from '@/lib/matchDetail'

/**
 * The opening hand in the app's order (Bottomed and T1 draw pills, the draw
 * last) and the hands sent back (0.x `MatchGame.vue`). A manual match's hand
 * has an Add or Edit button.
 */
const props = withDefaults(
  defineProps<{
    game: GameDetailView
    manual?: boolean
  }>(),
  { manual: false },
)

const emit = defineEmits<{ edit: [] }>()

const tiles = computed(() => openingHandTiles(props.game))
const gridClass = computed(() => handGridClass(props.game.openingHand.length))
</script>

<template>
  <!-- Opening hand -->
  <Card class="gap-0 py-0">
    <header class="panel-header">
      <Hand :size="13" class="text-muted-foreground" />
      <h3 class="text-[11px] font-semibold tracking-widest uppercase">{{ openingHandTitle(game.localMulligans) }}</h3>
      <span class="font-mono text-[11px] text-muted-foreground">({{ game.openingHand.length }})</span>
      <Button v-if="manual" variant="ghost" size="sm" class="h-6 px-1.5 text-[11px]" @click="emit('edit')">
        <PencilLine :size="11" />
        {{ game.openingHand.length ? 'Edit' : 'Add' }}
      </Button>
      <!-- The game's own actions (game log, replay). -->
      <div class="ml-auto flex items-center gap-1">
        <slot name="actions" />
      </div>
    </header>

    <div v-if="game.openingHand.length" class="grid gap-1.5 p-4" :class="gridClass">
      <div v-for="(tile, i) in tiles" :key="`opening_${i}`" class="flex flex-col items-center gap-1">
        <div class="aspect-[63/88] w-full overflow-hidden rounded-md shadow-sm" :class="tile.card.bottomed ? 'opacity-60' : ''">
          <CardTile :name="tile.card.name" :image="tile.card.image" :local="tile.card.localImage" />
        </div>
        <CardTilePill v-if="tile.pill === 'bottomed'" tone="bottomed">Bottomed</CardTilePill>
        <CardTilePill v-else-if="tile.pill === 'draw'" tone="draw">T1 draw</CardTilePill>
      </div>
    </div>
    <p v-else class="m-4 rounded-md border border-dashed px-4 py-5 text-center text-xs text-muted-foreground italic">
      {{ manual ? 'No hand recorded for this game.' : 'No opening hand captured.' }}
    </p>
  </Card>

  <!-- Mulliganed hands -->
  <Card v-if="game.mulliganedHands.length" class="gap-0 py-0">
    <header class="panel-header">
      <Undo2 :size="13" class="text-muted-foreground" />
      <h3 class="text-[11px] font-semibold tracking-widest uppercase">Mulliganed hands</h3>
      <span class="font-mono text-[11px] text-muted-foreground">({{ game.mulliganedHands.length }})</span>
    </header>

    <div class="flex flex-col gap-4 p-4">
      <div v-for="(hand, hi) in game.mulliganedHands" :key="`mull_${hi}`" class="flex flex-col gap-1.5">
        <p v-if="game.mulliganedHands.length > 1" class="font-mono text-[11px] text-muted-foreground">Hand {{ hi + 1 }}</p>
        <p
          v-if="hand.length === 0"
          class="rounded-md border border-dashed px-4 py-3 text-center text-xs text-muted-foreground italic"
        >
          Not recorded.
        </p>
        <div v-else class="grid grid-cols-7 gap-1.5">
          <div
            v-for="(card, ci) in hand"
            :key="`mull_${hi}_${ci}`"
            class="aspect-[63/88] shrink-0 overflow-hidden rounded-md border border-transparent"
          >
            <CardTile :name="card.name" :image="card.image" :local="card.localImage" />
          </div>
        </div>
      </div>
    </div>
  </Card>
</template>
