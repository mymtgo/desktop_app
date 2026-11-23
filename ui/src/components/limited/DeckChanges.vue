<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import CardHoverPreview from '@/components/cards/CardHoverPreview.vue'
import { computed } from 'vue'
import type { RegisteredVersionView } from '@/bindings'
import ManaPips from '@/components/limited/ManaPips.vue'
import type { LimitedCards } from '@/lib/limited'
import { deckChanges } from '@/lib/limitedPages'

/**
 * What changed between the selected build and the one before it, in the
 * zone list style: signed count cell, art crop, name, colour pips (0.x
 * `DeckChanges`).
 */
const props = defineProps<{ versions: RegisteredVersionView[]; cards: LimitedCards; selected: number | null }>()

const changes = computed(() => deckChanges(props.versions, props.cards, props.selected))
</script>

<template>
  <div class="flex flex-col overflow-hidden rounded-lg border border-black/60 bg-card">
    <div class="flex items-baseline justify-between gap-2 px-4 py-3">
      <h2 class="text-sm font-semibold">Changes</h2>
      <span v-if="changes.previous !== null" class="text-xs text-muted-foreground">
        v{{ selected }} vs v{{ changes.previous }}
      </span>
    </div>

    <div v-if="changes.state === 'none'" class="border-t px-4 py-3 text-xs text-muted-foreground">
      No registered deck yet.
    </div>
    <div v-else-if="changes.state === 'first'" class="border-t px-4 py-3 text-xs text-muted-foreground">
      First build after deck construction.
    </div>
    <div v-else-if="changes.state === 'identical'" class="border-t px-4 py-3 text-xs text-muted-foreground">
      Identical to v{{ changes.previous }}.
    </div>

    <div v-for="section in changes.sections" :key="section.key">
      <h3
        class="flex items-baseline justify-between gap-2 border-y py-2 pr-3 pl-4 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase"
      >
        <span>{{ section.label }}</span>
        <span class="tabular-nums">{{ section.rows.length }}</span>
      </h3>
      <div class="divide-y text-xs">
        <div v-for="row in section.rows" :key="row.key" class="flex items-center text-sm hover:bg-muted/40">
          <span
            class="w-8 shrink-0 border-r bg-black/20 px-2 py-1 text-center font-semibold tabular-nums"
            :class="row.sign === '+' ? 'text-emerald-400' : 'text-rose-400'"
          >
            {{ row.sign }}{{ row.quantity }}
          </span>
          <CardHoverPreview :image="row.image" :local-image="row.localImage" :name="row.name">
            <span class="flex min-w-0 grow items-center">
              <span class="h-7 w-7 shrink-0 overflow-hidden bg-black/20">
                <CardImage v-if="row.artCrop" :src="row.artCrop" :local="row.localArtCrop" :alt="row.name" class="h-full w-full object-cover" />
              </span>
              <span class="min-w-0 grow truncate px-2">{{ row.name }}</span>
            </span>
          </CardHoverPreview>
          <span class="flex shrink-0 items-center px-3"><ManaPips v-if="row.colors" :colors="row.colors" /></span>
        </div>
      </div>
    </div>
  </div>
</template>
