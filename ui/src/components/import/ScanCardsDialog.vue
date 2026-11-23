<script setup lang="ts">
import { ref, watch } from 'vue'
import { commands, type HistoryScanCards, type HistoryScanMatch } from '@/bindings'
import { Badge } from '@/components/ui/badge'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Spinner } from '@/components/ui/spinner'
import { unwrap } from '@/lib/commands'
import { formatScanDate } from '@/lib/historyImport'

/** The cards a scanned match's game log names (0.x cards modal). */
const props = defineProps<{ match: HistoryScanMatch | null }>()
const open = defineModel<boolean>('open', { required: true })

const loading = ref(false)
const cards = ref<HistoryScanCards | null>(null)

watch(
  () => [open.value, props.match?.historyId] as const,
  async ([isOpen, historyId]) => {
    if (!isOpen || historyId === undefined) return
    loading.value = true
    cards.value = null
    try {
      const loaded = unwrap(await commands.historyScanCards(historyId))
      // Another match may have been opened while this one loaded.
      if (props.match?.historyId === historyId) cards.value = loaded
    } catch {
      // As 0.x: a failed load leaves the dialog empty.
    } finally {
      if (props.match?.historyId === historyId) loading.value = false
    }
  },
  { immediate: true },
)
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-h-[80vh] max-w-lg overflow-y-auto">
      <DialogHeader>
        <DialogTitle>
          Cards seen - {{ match?.opponent }}
          <span class="font-normal text-muted-foreground">
            ({{ match?.formatDisplay }}, {{ match ? formatScanDate(match.startedAtMs) : '' }})
          </span>
        </DialogTitle>
        <DialogDescription>Cards played or revealed during this match.</DialogDescription>
      </DialogHeader>

      <div v-if="loading" class="flex justify-center py-8">
        <Spinner class="size-6" />
      </div>

      <div v-else-if="cards" class="space-y-4">
        <div v-if="cards.localCards.length > 0">
          <h4 class="mb-2 text-xs font-medium text-muted-foreground uppercase">
            Your cards ({{ cards.localCards.length }})
          </h4>
          <div class="flex flex-wrap gap-1">
            <Badge v-for="card in cards.localCards" :key="card.mtgoId" variant="secondary" class="text-xs">
              {{ card.name }}
            </Badge>
          </div>
        </div>
        <div v-if="cards.opponentCards.length > 0">
          <h4 class="mb-2 text-xs font-medium text-muted-foreground uppercase">
            Opponent cards ({{ cards.opponentCards.length }})
          </h4>
          <div class="flex flex-wrap gap-1">
            <Badge v-for="card in cards.opponentCards" :key="card.mtgoId" variant="outline" class="text-xs">
              {{ card.name }}
            </Badge>
          </div>
        </div>
        <p
          v-if="cards.localCards.length === 0 && cards.opponentCards.length === 0"
          class="py-4 text-center text-sm text-muted-foreground"
        >
          No card data available for this match.
        </p>
      </div>
    </DialogContent>
  </Dialog>
</template>
