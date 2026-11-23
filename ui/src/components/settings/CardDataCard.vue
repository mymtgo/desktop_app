<script setup lang="ts">
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, ref } from 'vue'
import SettingsCard from '@/components/SettingsCard.vue'
import { Button } from '@/components/ui/button'
import { commands } from '@/bindings'
import { cardDataSummary, downloadCardDataLabel } from '@/lib/cards'
import { unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'

// 0.x CardDataCard. "Download card data" scans for missing cards, then
// fills them, and answers when done; `CardsChanged` refetches the counts.
const queryClient = useQueryClient()
const { data: status } = useQuery({
  queryKey: keys.cardData,
  queryFn: async () => unwrap(await commands.cardDataStatus()),
})
const running = ref(false)
const failure = ref<string | null>(null)
const summary = computed(() => (status.value ? cardDataSummary(status.value) : ''))

async function download() {
  running.value = true
  failure.value = null
  try {
    const answer = await commands.downloadCardData()
    if (answer.status === 'error') failure.value = answer.error.message
  } finally {
    running.value = false
    void queryClient.invalidateQueries({ queryKey: keys.cardData })
  }
}
</script>

<template>
  <SettingsCard
    title="Card data"
    description="Download names, artwork and details for the cards your decks and matches reference."
  >
    <template #details>
      <p class="pt-2 text-xs font-medium">Card details</p>
      <p class="text-xs text-muted-foreground">{{ summary }}</p>
      <p v-if="failure" class="text-xs text-destructive">{{ failure }}</p>
    </template>
    <Button size="sm" variant="outline" :disabled="running" @click="download">
      {{ downloadCardDataLabel(running) }}
    </Button>
  </SettingsCard>
</template>
