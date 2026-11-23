<script setup lang="ts">
// The opponent cards seen against an archetype (spec section 10) as a grid
// of thumbnails (the opening hand's tiles), with the ones in no typical
// list set apart under "Not in typical lists".
import { computed } from 'vue'
import type { SeenCardView } from '@/bindings'
import CardTile from '@/components/matches/CardTile.vue'
import { seenCaption, splitSeen } from '@/lib/metagame'

const props = defineProps<{ cards: SeenCardView[] }>()
const split = computed(() => splitSeen(props.cards))
const groups = computed(() => [
  { key: 'typical', title: null, cards: split.value.typical },
  { key: 'other', title: 'Not in typical lists', cards: split.value.other },
])
</script>

<template>
  <div class="flex flex-col gap-4">
    <p v-if="cards.length === 0" class="text-sm text-muted-foreground">No cards seen yet.</p>
    <template v-for="group in groups" :key="group.key">
      <section v-if="group.cards.length" class="flex flex-col gap-3">
        <header v-if="group.title" class="flex items-baseline gap-3">
          <h3 class="text-[11px] font-semibold tracking-widest uppercase">{{ group.title }}</h3>
          <span class="font-mono text-[11px] text-muted-foreground">({{ group.cards.length }})</span>
          <span class="ml-2 h-px flex-1 bg-border" />
        </header>
        <div class="grid grid-cols-[repeat(auto-fill,minmax(110px,1fr))] gap-1.5">
          <div v-for="card in group.cards" :key="card.key" class="flex min-w-0 flex-col items-center gap-1" :title="card.name">
            <div class="aspect-[63/88] w-full overflow-hidden rounded-md shadow-sm">
              <CardTile :name="card.name" :image="card.image" :local="card.localImage" loading="lazy" />
            </div>
            <span class="max-w-full truncate text-[11px] text-muted-foreground tabular-nums">
              {{ seenCaption(card) }}
            </span>
          </div>
        </div>
      </section>
    </template>
  </div>
</template>
