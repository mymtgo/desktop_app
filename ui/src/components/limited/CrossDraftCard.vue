<script setup lang="ts">
import { computed } from 'vue'
import type { CrossDraftView, LimitedCardView } from '@/bindings'
import { Skeleton } from '@/components/ui/skeleton'
import { crossDraftRows } from '@/lib/limited'

/**
 * How the selected card fared in the user's other drafts of the set (0.x
 * `CrossDraftCard`). `stats` is undefined while it loads.
 */
const props = defineProps<{
  card: LimitedCardView | null
  setLabel: string | null
  stats: CrossDraftView | null | undefined
  packSize: number
}>()

const rows = computed(() => (props.stats?.stats ? crossDraftRows(props.stats.stats, props.packSize) : null))
</script>

<template>
  <div class="flex flex-col gap-2 rounded-lg border border-black/60 bg-card p-4 text-xs">
    <span class="font-semibold">
      <template v-if="card">{{ card.name }} in your other {{ setLabel ?? '' }} drafts</template>
      <template v-else>Select a card to compare across drafts</template>
    </span>
    <template v-if="card">
      <div v-if="stats === undefined" class="flex flex-col gap-2">
        <Skeleton class="h-3 w-3/4" />
        <Skeleton class="h-3 w-1/2" />
        <Skeleton class="h-3 w-2/3" />
      </div>
      <div v-else-if="!rows" class="text-muted-foreground">Never seen in another {{ setLabel ?? '' }} draft.</div>
      <dl v-else class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
        <template v-for="row in rows" :key="row.label">
          <dt class="text-muted-foreground">{{ row.label }}</dt>
          <dd class="tabular-nums">
            {{ row.value }} <span v-if="row.extra" class="text-muted-foreground">{{ row.extra }}</span>
          </dd>
        </template>
      </dl>
    </template>
  </div>
</template>
