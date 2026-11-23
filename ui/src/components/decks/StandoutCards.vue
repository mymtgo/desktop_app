<script setup lang="ts">
import type { StandoutView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import { Card, CardContent } from '@/components/ui/card'

// The dashboard's six picks (0.x `StandoutCards.vue`).
const props = defineProps<{ standout: StandoutView }>()

const cards = [
  { key: 'topPerformer', title: 'Top Performer', emptyText: 'Not enough data' },
  { key: 'mostCast', title: 'Most Cast', emptyText: 'Not enough data' },
  { key: 'mostSeen', title: 'Most Seen', emptyText: 'Not enough data' },
  { key: 'mostPlayedLand', title: 'Most Played Land', emptyText: 'Not enough data' },
  { key: 'mostSidedIn', title: 'Most Sided In', emptyText: 'No sideboard data' },
  { key: 'mostSidedOut', title: 'Most Sided Out', emptyText: 'No sideboard data' },
] as const
</script>

<template>
  <div class="grid grid-cols-6 gap-4">
    <Card v-for="meta in cards" :key="meta.key" class="gap-0 overflow-hidden py-0">
      <CardContent class="flex h-full flex-col gap-2 p-4">
        <p class="text-xs font-medium tracking-wide text-muted-foreground uppercase">{{ meta.title }}</p>
        <template v-if="props.standout[meta.key]">
          <!-- A fixed card shape, so a tight row never squashes the picture. -->
          <CardImage
            :src="props.standout[meta.key]!.image"
            :local="props.standout[meta.key]!.localImage"
            :alt="props.standout[meta.key]!.name"
            loading="lazy"
            class="aspect-[63/88] w-full shrink-0 rounded-[4.5%/3.3%] object-cover shadow-sm"
          />
          <div class="flex min-w-0 flex-col">
            <span class="truncate text-sm font-medium" :title="props.standout[meta.key]!.name">{{ props.standout[meta.key]!.name }}</span>
            <span class="text-xs text-muted-foreground">{{ props.standout[meta.key]!.stat }}</span>
          </div>
        </template>
        <!-- No pick: the note sits in the middle of the space the card would take. -->
        <div v-else class="flex flex-1 items-center justify-center">
          <span class="text-xs text-muted-foreground/70 italic">{{ meta.emptyText }}</span>
        </div>
      </CardContent>
    </Card>
  </div>
</template>
