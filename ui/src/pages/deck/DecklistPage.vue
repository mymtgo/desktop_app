<script setup lang="ts">
import { Camera, Download, Loader2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { commands, type DeckScreenshotView } from '@/bindings'
import CurveAndColours from '@/components/decks/CurveAndColours.vue'
import DeckList from '@/components/decks/DeckList.vue'
import HypergeometricCalculator from '@/components/decks/HypergeometricCalculator.vue'
import DeckScreenshot from '@/components/screenshots/DeckScreenshot.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useCurrentDeckHeader, useDeckPage } from '@/composables/useDeckPage'
import { loadDeckScreenshot, useOffscreenScreenshot } from '@/composables/useOffscreenScreenshot'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { decklistOrgUrl, dekExportFailedToast, dekExportToast, openRegistrationLink } from '@/lib/decklist'
import { useDeckDecklist } from '@/lib/queries'
import { screenshotFileName } from '@/lib/screenshots'

const { deckId, version } = useDeckPage()
const { data: header } = useCurrentDeckHeader(useDeckBestOfContext().bestOf)
const { data: decklist, error } = useDeckDecklist(deckId, version)
const { add: addToast } = useToast()

const registrationUrl = computed(() =>
  decklist.value && header.value
    ? decklistOrgUrl(decklist.value.main, decklist.value.side, header.value.deck.formatLabel)
    : null,
)

async function openRegistration() {
  if (!registrationUrl.value) return
  const toast = await openRegistrationLink(registrationUrl.value, commands.openExternal)
  if (toast) addToast(toast)
}

const exporting = ref(false)

async function downloadDek() {
  if (exporting.value) return
  exporting.value = true
  try {
    const toast = dekExportToast(unwrap(await commands.exportDeckDek(deckId.value, version.value)))
    if (toast) addToast(toast)
  } catch (failure) {
    addToast(dekExportFailedToast(failure))
  } finally {
    exporting.value = false
  }
}

// "Share Deck" (0.x `copyDeckScreenshot`): the deck as an image on the
// clipboard, or "Save image" when the clipboard refuses.
const screenshot = useOffscreenScreenshot<DeckScreenshotView>()
const { shot, busy: generating, saveOffered, saving } = screenshot
const shotTarget = screenshot.target

function shareDeck() {
  void screenshot.take(
    () => loadDeckScreenshot(deckId.value),
    screenshotFileName(header.value?.deck.name, 'deck'),
  )
}
</script>

<template>
  <div class="p-3 lg:p-4">
    <div class="mb-4 flex items-center justify-end gap-2">
      <button
        v-if="saveOffered"
        :disabled="saving"
        class="inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
        @click="screenshot.save"
      >
        <Download class="size-4" />
        Save image
      </button>
      <button
        :disabled="generating"
        class="inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
        @click="shareDeck"
      >
        <Loader2 v-if="generating" class="size-4 animate-spin" />
        <Camera v-else class="size-4" />
        {{ generating ? 'Generating...' : 'Share Deck' }}
      </button>
      <button
        :disabled="exporting"
        class="inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50"
        @click="downloadDek"
      >
        <Loader2 v-if="exporting" class="size-4 animate-spin" />
        <Download v-else class="size-4" />
        {{ exporting ? 'Saving…' : 'Download .dek' }}
      </button>
      <!-- Opened in the browser by the app: the webview cannot open new windows. -->
      <button
        v-if="registrationUrl"
        type="button"
        class="inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm text-muted-foreground transition-colors hover:text-foreground"
        @click="openRegistration"
      >
        Deck Registration
      </button>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load the decklist: {{ errorMessage(error) }}</p>

    <div v-else-if="decklist" class="grid grid-cols-4 gap-4">
      <div class="col-span-3">
        <DeckList :commanders="decklist.commanders" :maindeck="decklist.main" :sideboard="decklist.side" />
      </div>
      <div class="col-span-1 flex flex-col gap-4">
        <CurveAndColours
          :curve="decklist.curve"
          :colours="decklist.colours"
          :total="decklist.nonlandTotal"
          scope="maindeck, nonland"
        />

        <!-- Hypergeometric draw odds -->
        <HypergeometricCalculator :maindeck="decklist.main" />
      </div>
    </div>

    <div v-else class="grid grid-cols-4 gap-4">
      <div class="col-span-3 flex flex-wrap gap-2">
        <Skeleton v-for="n in 8" :key="n" class="h-[209px] w-[150px] rounded-[7px]" />
      </div>
      <div class="col-span-1 flex flex-col gap-4">
        <Skeleton class="h-[150px] w-full" />
        <Skeleton class="h-[200px] w-full" />
      </div>
    </div>

    <!-- Off screen: the capture's source. -->
    <div v-if="shot" style="position: fixed; top: -9999px; left: -9999px; pointer-events: none">
      <DeckScreenshot ref="shotTarget" :data="shot" />
    </div>
  </div>
</template>
