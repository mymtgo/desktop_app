<script setup lang="ts">
import { ArrowLeft, ArrowRight, Clock, Eye, Layers } from 'lucide-vue-next'
import { computed } from 'vue'
import type { DraftPickView, SeenWheelView } from '@/bindings'
import CardThumb from '@/components/limited/CardThumb.vue'
import ReservationTrail from '@/components/limited/ReservationTrail.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger } from '@/components/ui/select'
import {
  cardFor,
  pickBadgeTime,
  pickGroups,
  pickHeading,
  pickTiles,
  stepPick,
  wheelSummary,
  type LimitedCards,
} from '@/lib/limited'

/**
 * One pick (0.x `PickDetail`): the pick select with previous and next, the
 * pack's badges, the pack grid (the pick first, padded to the pack size),
 * the reservation trail and the wheel summary.
 */
const props = defineProps<{
  pick: DraftPickView
  picks: DraftPickView[]
  cards: LimitedCards
  seenWheel: { [key in number]: SeenWheelView }
  /** Cards the pack opened with, so the grid keeps its height as picks shrink it. */
  packSize: number
}>()

const emit = defineEmits<{ select: [ordinal: number]; selectCard: [catalogId: number] }>()

const grid = computed(() => pickTiles(props.pick, props.cards, props.seenWheel, props.packSize))
const groups = computed(() => pickGroups(props.picks))
const previous = computed(() => stepPick(props.picks, props.pick.ordinal, -1))
const next = computed(() => stepPick(props.picks, props.pick.ordinal, 1))
const time = computed(() => pickBadgeTime(props.pick))
const wheel = computed(() => wheelSummary(props.pick, props.cards, props.seenWheel))

function step(target: number | null) {
  if (target !== null) emit('select', target)
}
</script>

<template>
  <div class="flex flex-col gap-4 rounded-lg">
    <div class="flex flex-wrap items-center gap-2">
      <Select :model-value="String(pick.ordinal)" @update:model-value="(value) => emit('select', Number(value))">
        <SelectTrigger size="sm" class="font-semibold" aria-label="Jump to a pick">
          <span>{{ pickHeading(pick) }}</span>
        </SelectTrigger>
        <SelectContent class="max-h-80">
          <SelectGroup v-for="[pack, list] in groups" :key="pack">
            <SelectLabel>Pack {{ pack }}</SelectLabel>
            <SelectItem v-for="option in list" :key="option.ordinal" :value="String(option.ordinal)">
              <span class="tabular-nums">Pick {{ option.pickNumber }}</span>
              <span class="truncate text-muted-foreground">
                {{ option.pickedCatalogId !== null ? cardFor(cards, option.pickedCatalogId).name : 'no pick' }}
              </span>
            </SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>

      <div class="flex items-center gap-1">
        <Button variant="ghost" size="icon-sm" :disabled="previous === null" aria-label="Previous pick" @click="step(previous)">
          <ArrowLeft class="size-3.5" />
        </Button>
        <Button variant="ghost" size="icon-sm" :disabled="next === null" aria-label="Next pick" @click="step(next)">
          <ArrowRight class="size-3.5" />
        </Button>
      </div>

      <div class="ml-auto flex flex-wrap items-center gap-2">
        <Badge variant="outline" class="text-muted-foreground"><Layers class="size-3" /> {{ pick.available.length }} cards</Badge>
        <Badge v-if="pick.direction !== null" variant="outline" class="text-muted-foreground">
          <component :is="pick.direction === 1 ? ArrowRight : ArrowLeft" class="size-3" /> passed
          {{ pick.direction === 1 ? 'right' : 'left' }}
        </Badge>
        <Badge v-if="time" variant="outline" class="text-muted-foreground"><Clock class="size-3" /> {{ time }}</Badge>
        <Badge v-if="pick.indecisive" variant="outline" class="border-amber-500/30 text-amber-300">
          <Eye class="size-3" /> {{ pick.reservations.length }} reservations
        </Badge>
      </div>
    </div>

    <Card class="gap-0 p-4">
      <CardContent class="p-0">
        <div class="grid grid-cols-7 gap-3">
          <button
            v-for="tile in grid.tiles"
            :key="tile.key"
            type="button"
            class="min-w-0 rounded-md focus:outline-none focus-visible:ring-2 focus-visible:ring-primary"
            :title="tile.card.name"
            @click="emit('selectCard', tile.card.catalogId)"
          >
            <CardThumb
              :card="tile.card"
              fluid
              :highlighted="tile.picked"
              :dimmed="tile.gone"
              :class="tile.wheeled && !tile.picked ? 'ring-1 ring-muted-foreground/30' : ''"
            >
              <span v-if="tile.seen" class="absolute right-1 bottom-1 rounded bg-black/70 px-1 text-[10px]">
                {{ tile.seen }}
              </span>
              <span
                v-if="tile.passed"
                class="absolute bottom-1 left-1 rounded bg-black/70 px-1 text-[10px] text-muted-foreground"
              >
                {{ tile.passed }}
              </span>
              <Badge v-if="tile.picked" variant="default" class="absolute bottom-1 left-1">Pick</Badge>
              <Badge
                v-else-if="tile.wheeled"
                variant="outline"
                class="absolute bottom-1 left-1 bg-background/85 text-muted-foreground"
              >
                Wheeled
              </Badge>
            </CardThumb>
          </button>
          <div
            v-for="i in grid.fillers"
            :key="`filler-${i}`"
            class="aspect-[5/7] rounded-md border border-dashed border-border/40"
          />
        </div>
      </CardContent>
    </Card>

    <ReservationTrail :pick="pick" :cards="cards" />
    <p v-if="wheel" class="text-xs text-muted-foreground">{{ wheel }}</p>
  </div>
</template>
