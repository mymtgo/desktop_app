<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Download, RefreshCw } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { commands, type CardStatView } from '@/bindings'
import CardStatsHelp from '@/components/cards/CardStatsHelp.vue'
import CardStatsView from '@/components/cards/CardStatsView.vue'
import TrustSliderPopover from '@/components/cards/TrustSliderPopover.vue'
import CardStatScreenshot from '@/components/screenshots/CardStatScreenshot.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Skeleton } from '@/components/ui/skeleton'
import { Spinner } from '@/components/ui/spinner'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useCurrentDeckHeader, useDeckPage } from '@/composables/useDeckPage'
import { loadCardImage, useOffscreenScreenshot } from '@/composables/useOffscreenScreenshot'
import { useToast } from '@/composables/useToast'
import { useTrustSetting } from '@/composables/useTrustSetting'
import { effectiveFilters, filtersToQuery, DEFAULT_FILTERS, type CardStatsFilters } from '@/lib/cardStats/filters'
import type { Visibility } from '@/lib/cardStats/columns'
import { ALL } from '@/lib/cardStats/filters'
import { REGENERATE_FAILED_MESSAGE, regenerateMessage, trustMax } from '@/lib/cardStats/format'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'
import { useDeckCardStats } from '@/lib/queries'
import { boardChip, playDrawChip, screenshotFileName } from '@/lib/screenshots'
import { timeframeLabel } from '@/lib/timeframes'

const { deckId, version, timeframe, setTimeframe } = useDeckPage()
const { bestOf, bo1Mode } = useDeckBestOfContext()
const { data: header } = useCurrentDeckHeader(bestOf)
const queryClient = useQueryClient()
const { add: toast } = useToast()

// The filters reload the stats (the query key holds them); another deck starts afresh.
const filters = ref<CardStatsFilters>({ ...DEFAULT_FILTERS })
watch(deckId, () => {
  filters.value = { ...DEFAULT_FILTERS }
})

// Bo1 has no postboard: a board chosen on a Bo3 deck does not carry over.
const shownFilters = computed(() => effectiveFilters(filters.value, bo1Mode.value))
const query = computed(() => filtersToQuery(shownFilters.value, timeframe.value))
const { data: payload, error } = useDeckCardStats(deckId, version, query, bestOf)

const trust = useTrustSetting()
watch(
  () => payload.value?.trust,
  (stored) => {
    if (stored !== undefined) trust.seed(stored)
  },
  { immediate: true },
)
const max = computed(() => trustMax(payload.value?.deckWinrate.games ?? 0))

// "Regenerate" is refused for an archived deck (deleted on MTGO).
const readonly = computed(() => header.value?.deck.archived === true)
const READONLY_TITLE = 'Deck deleted on MTGO, read-only'
// Only MTGO matches are recounted: Arena and Endstep card stats come from the
// game sync, so "Regenerate" would recount nothing there.
const regenerable = computed(() => header.value?.platform === 'mtgo')

const regenerateOpen = ref(false)
const regenerating = ref(false)

async function regenerate() {
  regenerating.value = true
  try {
    const count = unwrap(await commands.regenerateCardStats(deckId.value))
    regenerateOpen.value = false
    toast({ type: 'success', title: 'Card stats regenerating', message: regenerateMessage(count) })
    void queryClient.invalidateQueries({ queryKey: keys.allDecks })
  } catch {
    toast({ type: 'error', title: 'Failed', message: REGENERATE_FAILED_MESSAGE })
  } finally {
    regenerating.value = false
  }
}

// The row menu's "Copy screenshot" (0.x `DeckCardStats.copyScreenshot`):
// the card and its stats with the filters as chips; the image is read as a
// `data:` URL first.
type CardShot = {
  stat: CardStatView
  columns: Visibility
  image: string | null
  archetypeName: string | null
  archetypeColorIdentity: string | null
  boardLabel: string | null
  playDrawLabel: string | null
  timeframeLabel: string
}
const screenshot = useOffscreenScreenshot<CardShot>()
const { shot, saveOffered, saving } = screenshot
const shotTarget = screenshot.target

function copyScreenshot(stat: CardStatView, columns: Visibility) {
  const current = shownFilters.value
  const archetype =
    current.archetype === ALL
      ? null
      : (payload.value?.archetypes.find((option) => String(option.id) === current.archetype) ?? null)
  void screenshot.take(
    async () => ({
      stat,
      columns,
      image: await loadCardImage(stat.oracleId),
      archetypeName: archetype?.name ?? null,
      archetypeColorIdentity: archetype?.colorIdentity ?? null,
      boardLabel: boardChip(current.board),
      playDrawLabel: playDrawChip(current.playDraw),
      timeframeLabel: timeframeLabel(timeframe.value),
    }),
    screenshotFileName(stat.name, 'card'),
  )
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col gap-3 p-3 lg:p-4">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />

      <div class="flex flex-wrap items-center gap-2">
        <Button v-if="saveOffered" size="sm" variant="outline" :disabled="saving" @click="screenshot.save">
          <Download class="mr-1.5 size-3.5" />
          Save image
        </Button>
        <TrustSliderPopover :model-value="trust.value.value" :max="max" @update:model-value="trust.setValue" @reset="trust.reset" />

        <Button
          v-if="regenerable"
          variant="ghost"
          size="sm"
          class="bevel gap-1.5 border border-black/60 px-2.5 py-4 text-xs text-muted-foreground"
          :disabled="regenerating || readonly"
          :title="readonly ? READONLY_TITLE : undefined"
          @click="regenerateOpen = true"
        >
          <RefreshCw class="size-3.5" :class="regenerating ? 'animate-spin' : ''" />
          <span class="hidden lg:inline">Regenerate</span>
        </Button>

        <CardStatsHelp />
      </div>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load card stats: {{ errorMessage(error) }}</p>

    <CardStatsView
      v-else-if="payload"
      v-model:filters="filters"
      :payload="payload"
      :trust="trust.value.value"
      :bo1="bo1Mode"
      screenshots
      @screenshot="copyScreenshot"
    />

    <Card v-else class="gap-0 overflow-hidden p-0">
      <CardContent class="flex flex-col gap-2 px-4 py-4">
        <Skeleton class="h-8 w-full" />
        <Skeleton class="h-8 w-full" />
        <Skeleton class="h-8 w-full" />
        <Skeleton class="h-8 w-3/4" />
      </CardContent>
    </Card>

    <div v-if="shot" style="position: fixed; top: -9999px; left: -9999px; pointer-events: none">
      <CardStatScreenshot
        ref="shotTarget"
        :stat="shot.stat"
        :visible-columns="shot.columns"
        :timeframe-label="shot.timeframeLabel"
        :archetype-name="shot.archetypeName"
        :archetype-color-identity="shot.archetypeColorIdentity"
        :board-label="shot.boardLabel"
        :play-draw-label="shot.playDrawLabel"
        :image-data-url="shot.image"
      />
    </div>

    <Dialog v-model:open="regenerateOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Regenerate card stats?</DialogTitle>
          <DialogDescription>
            This will recompute card stats for every game across all matches in this deck. Existing card stat rows will
            be replaced. Live matches process in the background and may take a moment to finish.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter class="gap-2 sm:gap-0">
          <Button variant="outline" :disabled="regenerating" @click="regenerateOpen = false">Cancel</Button>
          <Button :disabled="regenerating" @click="regenerate">
            <Spinner v-if="regenerating" class="mr-2 size-4" />
            {{ regenerating ? 'Queuing...' : 'Regenerate' }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
