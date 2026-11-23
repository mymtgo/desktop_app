<script setup lang="ts">
// A deck card's badges: the format, Archived, and the archetype (or
// Unclassified for a live deck without one). The card lays them out.
import type { DeckView } from '@/bindings'
import { Badge } from '@/components/ui/badge'

defineProps<{ deck: DeckView }>()
</script>

<template>
  <Badge variant="outline" class="shrink-0 py-0 text-xs">{{ deck.formatLabel }}</Badge>
  <Badge v-if="deck.archived" variant="destructive" class="shrink-0 py-0 text-xs">Archived</Badge>
  <Badge v-if="deck.archetype" variant="secondary" class="min-w-0 shrink py-0 text-xs">
    <span class="truncate">{{ deck.archetype.name }}</span>
  </Badge>
  <Badge v-else-if="!deck.archived" variant="warning" class="shrink-0 py-0 text-xs">Unclassified</Badge>
</template>
