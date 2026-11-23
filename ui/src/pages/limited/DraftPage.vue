<script setup lang="ts">
import { BookOpen } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import CrossDraftCard from '@/components/limited/CrossDraftCard.vue'
import ManaPips from '@/components/limited/ManaPips.vue'
import CardImageGroups from '@/components/limited/CardImageGroups.vue'
import PickDetail from '@/components/limited/PickDetail.vue'
import PickNoteEditor from '@/components/limited/PickNoteEditor.vue'
import SignalsPanel from '@/components/limited/SignalsPanel.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/commands'
import { cardFor, draftSubtitle, formatSeconds, limitedPath, NO_VALUE, selectPick, signedSeconds } from '@/lib/limited'
import { wholePool } from '@/lib/limitedPages'
import { useLimitedCrossDrafts, useLimitedDeck, useLimitedDraft, useLimitedEvent } from '@/lib/queries'
import { useLimitedKey } from '@/pages/limited/useLimitedKey'

/**
 * The draft review (0.x `pages/limited/Draft.vue`): the header and its
 * stats, the colour signals, the selected pick (`?pick=`, else the first)
 * with its note and the cross-draft card. Sealed has no picks: it goes to
 * the Deck page.
 */
const route = useRoute()
const router = useRouter()
const key = useLimitedKey()

const { data: event } = useLimitedEvent(key)
const { data: draft, error } = useLimitedDraft(key)

// Arena logs no picks for a draft: its page is the pool and a note.
const picksMissing = computed(() => event.value?.picksRecorded === false)
const { data: evolution } = useLimitedDeck(key, picksMissing)
const poolGroup = computed(() => {
  const cards = wholePool(evolution.value?.pool ?? { groups: [] }, evolution.value?.cards ?? {})
  return cards.length > 0 ? [{ key: 'pool', label: 'Card pool', cards }] : []
})

const review = computed(() => draft.value?.review ?? null)

watch(
  draft,
  (shown) => {
    if (shown?.redirectToDeck) void router.replace(limitedPath(key.value, 'deck'))
  },
  { immediate: true },
)

/** The `?pick=` ordinal, when it is a number. */
const requested = computed(() => {
  const value = Number(route.query.pick)
  return Number.isInteger(value) && value > 0 ? value : null
})

const selected = computed(() => (review.value ? selectPick(review.value.picks, requested.value) : null))
const pick = computed(() => review.value?.picks.find((candidate) => candidate.ordinal === selected.value) ?? null)

/** A card clicked in the grid; cleared on a pick change. */
const selectedCard = ref<number | null>(null)
watch(selected, () => {
  selectedCard.value = null
})

/** Keep the URL shareable: the pick goes in the query, without a refetch. */
function select(ordinal: number) {
  void router.replace({ query: { ...route.query, pick: String(ordinal) } })
}

const comparedCard = computed(() => {
  if (!review.value) return null
  if (selectedCard.value !== null) return cardFor(review.value.cards, selectedCard.value)
  const picked = pick.value?.pickedCatalogId ?? null
  if (picked !== null) return cardFor(review.value.cards, picked)
  return null
})

const comparedId = computed(() => comparedCard.value?.catalogId ?? null)
const { data: crossDrafts } = useLimitedCrossDrafts(key)
// undefined while the draft's map loads; null for a card it has none for.
const crossDraft = computed(() => {
  if (comparedId.value === null || crossDrafts.value === undefined) return undefined
  return crossDrafts.value?.cards.find((card) => card.catalogId === comparedId.value) ?? null
})

const subtitle = computed(() => (review.value ? draftSubtitle(review.value.header, event.value?.setLabel ?? null) : ''))
</script>

<template>
  <div class="flex flex-col gap-4 p-3">
    <p v-if="error" class="text-sm text-destructive">Could not load the draft: {{ errorMessage(error) }}</p>

    <template v-else-if="!draft || draft.redirectToDeck">
      <Skeleton class="h-12 w-full" />
      <Skeleton class="h-20 w-full" />
      <Skeleton class="h-96 w-full" />
    </template>

    <template v-else-if="!review && picksMissing">
      <div class="flex flex-col">
        <h1 class="text-base font-semibold tracking-tight">Draft review</h1>
        <p class="text-sm text-muted-foreground">Arena does not log picks for this draft.</p>
      </div>
      <CardImageGroups :groups="poolGroup" empty-text="No pool recorded for this event." />
    </template>

    <div v-else-if="!review" class="flex flex-col items-center gap-2 py-16 text-center">
      <BookOpen class="size-10 text-muted-foreground/40" />
      <p class="font-medium">No draft recorded for this event.</p>
      <p class="text-sm text-muted-foreground">Picks appear here once mymtgo sees a draft for this league.</p>
    </div>

    <template v-else>
      <div class="flex flex-wrap items-start justify-between gap-4">
        <div class="flex flex-col">
          <h1 class="text-base font-semibold tracking-tight">Draft review</h1>
          <p class="text-sm text-muted-foreground">{{ subtitle }}</p>
        </div>
        <div class="flex items-center gap-6 text-right">
          <div class="flex flex-col">
            <span class="text-[10px] tracking-wide text-muted-foreground uppercase">avg pick</span>
            <span class="text-lg font-bold tabular-nums">{{ formatSeconds(review.header.avgPickSeconds) }}</span>
          </div>
          <div class="flex flex-col">
            <span class="text-[10px] tracking-wide text-muted-foreground uppercase">timer margin</span>
            <span class="text-lg font-bold tabular-nums">{{ signedSeconds(review.header.avgMarginSeconds) }}</span>
          </div>
          <div class="flex flex-col">
            <span class="text-[10px] tracking-wide text-muted-foreground uppercase">indecision</span>
            <span class="text-lg font-bold tabular-nums">
              {{ review.header.indecisiveCount }}/{{ review.header.picksMade }}
            </span>
          </div>
          <div class="flex flex-col">
            <span class="text-[10px] tracking-wide text-muted-foreground uppercase">colours picked</span>
            <span v-if="review.header.colorsPicked.length" class="flex items-center gap-2 text-sm font-semibold tabular-nums">
              <span v-for="entry in review.header.colorsPicked" :key="entry.color" class="inline-flex items-center gap-1">
                <ManaPips :colors="entry.color === 'C' ? '' : entry.color" size="md" />{{ entry.count }}
              </span>
            </span>
            <span v-else class="text-lg font-bold">{{ NO_VALUE }}</span>
          </div>
        </div>
      </div>

      <Card class="gap-0 py-0">
        <SignalsPanel :signals="review.signals" />
      </Card>

      <div v-if="pick" class="grid gap-4 lg:grid-cols-[1fr_340px]">
        <PickDetail
          :pick="pick"
          :picks="review.picks"
          :cards="review.cards"
          :seen-wheel="review.seenWheel"
          :pack-size="review.header.packSize"
          @select="select"
          @select-card="(id) => (selectedCard = id)"
        />

        <div class="flex flex-col gap-4">
          <Card v-if="event?.draftToken" class="gap-0 p-1">
            <CardContent class="p-2">
              <PickNoteEditor :draft-token="event.draftToken" :pick="pick" />
            </CardContent>
          </Card>
          <CrossDraftCard
            :card="comparedCard"
            :set-label="event?.setLabel ?? null"
            :stats="crossDraft"
            :pack-size="review.header.packSize"
          />
        </div>
      </div>
    </template>
  </div>
</template>
