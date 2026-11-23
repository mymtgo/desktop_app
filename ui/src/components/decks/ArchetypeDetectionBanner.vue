<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Loader2, Sparkles } from 'lucide-vue-next'
import { computed, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { useArchetypeDetection } from '@/composables/useArchetypeDetection'
import { keys } from '@/lib/keys'
import { useDeckPendingArchetypes } from '@/lib/queries'

/**
 * The deck Matches page's "Detect archetypes" banner (0.x
 * `ArchetypeDetectionBanner`), shown while the archetype filter is set:
 * re-run detection for the matches in the filter; the queue count is read
 * every 3 s while matches wait, and the lists refetch as it drops.
 */
const props = defineProps<{
  deckId: number
  /** `none` or an archetype id. */
  filter: string
  archived: boolean
}>()

const queryClient = useQueryClient()
const { detecting, queueDeck } = useArchetypeDetection()
const { data: pending } = useDeckPendingArchetypes(computed(() => props.deckId))

const hasPending = computed(() => (pending.value ?? 0) > 0)

watch(pending, (now, before) => {
  if (before !== undefined && now !== undefined && now < before) {
    void queryClient.invalidateQueries({ queryKey: keys.allDeckMatches })
  }
})
</script>

<template>
  <div class="flex flex-wrap items-center justify-between gap-3 rounded-md border border-border/60 bg-muted/40 px-3 py-2">
    <div class="flex items-center gap-2 text-xs">
      <Sparkles class="size-4 text-muted-foreground" />
      <span>Re-run archetype detection for matches in this filter.</span>
      <span
        v-if="hasPending"
        class="inline-flex items-center gap-1 rounded-full bg-background px-2 py-0.5 text-[11px] text-muted-foreground"
      >
        <Loader2 class="size-3 animate-spin" />
        {{ pending }} in queue
      </span>
    </div>
    <Button
      size="sm"
      :disabled="detecting || hasPending || archived"
      :title="archived ? 'Deck deleted on MTGO, read-only' : undefined"
      @click="queueDeck(deckId, filter)"
    >
      <Loader2 v-if="detecting" class="size-3 animate-spin" />
      Detect archetypes
    </Button>
  </div>
</template>
