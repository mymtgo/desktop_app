<script setup lang="ts">
import { Download } from 'lucide-vue-next'
import { nextTick, ref, watch } from 'vue'
import { commands, type MatchupDetailView, type MatchupView } from '@/bindings'
import MatchupDrawer from '@/components/decks/MatchupDrawer.vue'
import MatchupScreenshot from '@/components/decks/MatchupScreenshot.vue'
import MatchupSpread from '@/components/decks/MatchupSpread.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useDeckPage } from '@/composables/useDeckPage'
import { useScreenshot } from '@/composables/useScreenshot'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { screenshotName } from '@/lib/matchups'
import { useDeckMatchups } from '@/lib/queries'
import { timeframeLabel } from '@/lib/timeframes'

// The deck Matchups page (0.x `decks/Matchups`): the spread, a row opens
// the drawer with that archetype's detail; a right click copies a
// screenshot of the matchup (0.x `DeckMatchups`).
const { deckId, version, timeframe, setTimeframe } = useDeckPage()
const { bestOf, bo1Mode } = useDeckBestOfContext()
const { data: matchups, error } = useDeckMatchups(deckId, version, timeframe, bestOf)

const selected = ref<MatchupView | null>(null)
watch([deckId, version, timeframe, bestOf], () => {
  selected.value = null
})

const { add: toast } = useToast()
const { capture, save, saving, saveOffered } = useScreenshot()
const shot = ref<{ matchup: MatchupView; detail: MatchupDetailView } | null>(null)
const shotElement = ref<InstanceType<typeof MatchupScreenshot> | null>(null)

async function copyScreenshot(matchup: MatchupView) {
  if (shot.value) return
  let detail: MatchupDetailView
  try {
    detail = unwrap(await commands.deckMatchupDetail(deckId.value, matchup.archetypeId, version.value, timeframe.value, bestOf.value))
  } catch {
    toast({ type: 'error', title: 'Screenshot failed', message: 'Could not load matchup data' })
    return
  }
  shot.value = { matchup, detail }
  try {
    await nextTick()
    const element = shotElement.value?.$el as HTMLElement | undefined
    if (element) await capture(element, screenshotName(matchup.name))
  } finally {
    shot.value = null
  }
}
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />
      <Button v-if="saveOffered" size="sm" variant="outline" :disabled="saving" @click="save">
        <Download class="mr-1.5 size-3.5" />
        Save image
      </Button>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load the matchups: {{ errorMessage(error) }}</p>
    <MatchupSpread
      v-else-if="matchups"
      :matchups="matchups"
      :bo1="bo1Mode"
      selectable
      screenshots
      @select="(row) => (selected = row)"
      @screenshot="copyScreenshot"
    />
    <div v-else class="space-y-2">
      <Skeleton v-for="n in 6" :key="n" class="h-8 w-full" />
    </div>

    <MatchupDrawer
      :deck-id="deckId"
      :matchup="selected"
      :timeframe="timeframe"
      :version="version"
      :best-of="bestOf"
      @close="selected = null"
    />

    <div v-if="shot" style="position: fixed; top: -9999px; left: -9999px; pointer-events: none">
      <MatchupScreenshot
        ref="shotElement"
        :matchup="shot.matchup"
        :detail="shot.detail"
        :timeframe-label="timeframeLabel(timeframe)"
      />
    </div>
  </div>
</template>
