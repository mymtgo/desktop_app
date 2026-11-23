<script setup lang="ts">
import { AlertTriangle } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { RouterLink, RouterView, useRoute, useRouter } from 'vue-router'
import DeckSidebar from '@/components/decks/DeckSidebar.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { provideDeckBestOf, storedBestOf, useDeckBestOf } from '@/composables/useDeckBestOf'
import { useCurrentDeckHeader, useDeckPage } from '@/composables/useDeckPage'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { usePageDetail } from '@/composables/useWindowTitle'
import { errorMessage } from '@/lib/commands'
import { deckLinkQuery, deckMatchesPath, deckSidebarPage } from '@/lib/deckRoute'

const route = useRoute()
const router = useRouter()
const { deckId, version, timeframe, setVersion } = useDeckPage()
// The header holds the kinds and is read for a kind: it starts on the
// remembered one (the kind `useDeckBestOf` gives until the kinds are read)
// and reads again only if the resolved kind differs.
const headerKind = ref(storedBestOf(deckId.value))
const { data: header, error } = useCurrentDeckHeader(headerKind)
const deckBestOf = useDeckBestOf(
  deckId,
  computed(() => (header.value ? (header.value.kinds ?? null) : undefined)),
)
provideDeckBestOf(deckBestOf)
watch(deckBestOf.bestOf, (kind) => (headerKind.value = kind), { immediate: true })

/** Bo1 has no game logs: these pages have no data there, so go to the dashboard. */
const BO1_EMPTY_PAGES = ['deck-game-stats', 'deck-sideboard-guides', 'deck-sideboard-guide']
watch(
  [deckBestOf.bo1Mode, () => route.name, () => !!header.value],
  ([bo1, name, loaded]) => {
    if (loaded && bo1 && BO1_EMPTY_PAGES.includes(String(name))) {
      void router.replace({ name: 'deck-dashboard', params: { id: deckId.value }, query: route.query })
    }
  },
  { immediate: true },
)
// A deck of the other platform (opened by URL) is not on this platform's
// pages: back to the deck list once both the deck and the context are read.
const { context } = usePlatformContext()
watch(
  [() => header.value?.platform, () => context.value?.current],
  ([deckPlatform, current]) => {
    if (deckPlatform !== undefined && current !== undefined && deckPlatform !== current) {
      void router.replace({ name: 'decks' })
    }
  },
  { immediate: true },
)
usePageDetail(() => header.value?.deck.name)

// A guide's editor sits under the Sideboard Guides item, a match under Matches.
const currentPage = computed(() => deckSidebarPage(String(route.name ?? '')))

/** The version select: on a match, show that version's matches instead. */
function chooseVersion(next: number | null) {
  if (route.name !== 'deck-match') {
    setVersion(next)
    return
  }
  void router.push(deckMatchesPath(deckId.value, deckLinkQuery({ version: next, timeframe: timeframe.value }, true)))
}
</script>

<template>
  <div v-if="header" class="flex min-h-0 flex-1 flex-col">
    <div
      v-if="header.deck.archived"
      class="flex items-center justify-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-4 py-1.5 text-xs text-amber-200"
    >
      <AlertTriangle class="size-3.5 shrink-0 text-amber-400" />
      <span>This deck has been archived. Historical match data is preserved.</span>
    </div>
    <RouterLink
      v-if="!header.deck.archetype"
      :to="{ name: 'deck-settings', params: { id: header.deck.id } }"
      class="flex items-center justify-center gap-2 border-b border-white/5 bg-white/5 px-4 py-1.5 text-xs text-white transition hover:bg-white/10"
    >
      <AlertTriangle class="size-3.5 shrink-0 text-amber-400" />
      <span>This deck is an unknown archetype. Click here to set it.</span>
    </RouterLink>
    <div class="flex min-h-0 flex-1">
      <div class="w-56 shrink-0">
        <DeckSidebar
          :header="header"
          :version="version"
          :timeframe="timeframe"
          :current-page="currentPage"
          @update:version="chooseVersion"
        />
      </div>
      <div class="flex min-h-0 flex-1 flex-col overflow-y-auto border-l border-white/5">
        <RouterView />
      </div>
    </div>
  </div>
  <div v-else-if="error" class="p-6 text-sm text-destructive">Could not load this deck: {{ errorMessage(error) }}</div>
  <div v-else-if="header === null" class="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
    <p class="text-sm font-medium">This deck no longer exists.</p>
    <RouterLink to="/decks" class="text-sm text-muted-foreground underline-offset-4 hover:underline">
      Back to decks
    </RouterLink>
  </div>
  <div v-else class="flex min-h-0 flex-1">
    <div class="w-56 shrink-0 border-r border-black/80 p-3"><Skeleton class="h-24 w-full" /></div>
    <div class="flex-1 p-4"><Skeleton class="h-40 w-full" /></div>
  </div>
</template>
