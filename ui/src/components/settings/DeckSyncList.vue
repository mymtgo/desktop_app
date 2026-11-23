<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { Search } from 'lucide-vue-next'
import { Badge } from '@/components/ui/badge'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { commands, events, type DeckSyncListView, type DeckSyncRowView } from '@/bindings'
import {
  deckSlotsText,
  deckSyncBlockedReason,
  deckSyncPlatformLabel,
  deckSyncSearchable,
  filterDeckSync,
  freePlanText,
  slotHeldText,
} from '@/lib/api'
import { useTauriEvents } from '@/composables/useTauriEvents'
import { errorMessage, unwrap } from '@/lib/commands'
import { relativeTime } from '@/lib/opponents'

// 0.x DeckSyncList. `linked` mirrors the account card above, which reports
// a sign-in the moment it completes.
const props = defineProps<{ linked: boolean }>()

const list = ref<DeckSyncListView | null>(null)
const search = ref('')
const savingId = ref<number | null>(null)
// The last deck toggled, so a refusal lands under the row that caused it.
const lastToggledId = ref<number | null>(null)
const error = ref<string | null>(null)
// The switch moves at once; the reloaded list confirms it, and a refused
// toggle rolls back with it.
const optimistic = ref<Record<number, boolean>>({})
const now = ref(Date.now())

const decks = computed(() => list.value?.decks ?? [])
const slots = computed(() => list.value?.slots ?? { limit: null, used: 0 })
const visibleDecks = computed(() => filterDeckSync(decks.value, search.value))

function isOn(deck: DeckSyncRowView): boolean {
  return optimistic.value[deck.id] ?? deck.cloudSyncEnabled
}

async function load() {
  try {
    list.value = unwrap(await commands.deckSyncList())
    now.value = Date.now()
  } catch (problem) {
    error.value = errorMessage(problem)
  }
}

async function toggle(deck: DeckSyncRowView, value: boolean) {
  optimistic.value = { ...optimistic.value, [deck.id]: value }
  savingId.value = deck.id
  lastToggledId.value = deck.id
  error.value = null
  try {
    unwrap(await commands.setDeckCloudSync(deck.id, value))
  } catch (problem) {
    error.value = errorMessage(problem)
  } finally {
    await load()
    optimistic.value = {}
    savingId.value = null
  }
}

watch(
  () => props.linked,
  () => void load(),
)

const listening = useTauriEvents(() => [events.decksChanged.listen(() => void load())])
onMounted(async () => {
  if (!(await listening.ready)) return
  await load()
})
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Deck sync</h2>
      <p class="text-xs text-muted-foreground">
        Pick the decks whose matches and leagues are backed up to your account. MTGO, MTG Arena and Endstep decks are
        all listed. Turning one off keeps its data in the cloud and holds its slot for 30 days.
      </p>
    </div>

    <div class="flex items-center justify-between gap-4">
      <span class="text-sm text-muted-foreground">Deck slots</span>
      <span class="text-sm tabular-nums">{{ deckSlotsText(slots) }}</span>
    </div>

    <div v-if="deckSyncSearchable(decks)" class="relative">
      <Search class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
      <Input v-model="search" placeholder="Search decks..." class="h-8 pl-7 text-xs" />
    </div>

    <p v-if="list !== null && decks.length === 0" class="text-sm text-muted-foreground">
      No decks yet. Play a match and your decks land here.
    </p>

    <p v-else-if="list !== null && visibleDecks.length === 0" class="text-sm text-muted-foreground">
      No deck matches "{{ search }}".
    </p>

    <div
      v-else-if="list !== null"
      class="max-h-80 divide-y divide-border overflow-auto rounded-lg border border-border bg-background"
    >
      <div v-for="deck in visibleDecks" :key="deck.id" class="flex flex-col gap-1 px-3 py-2.5">
        <div class="flex items-center justify-between gap-4">
          <div class="flex min-w-0 flex-col">
            <span class="flex min-w-0 items-center gap-1.5">
              <span class="truncate text-sm font-medium">{{ deck.name }}</span>
              <Badge variant="outline" class="text-[10px] text-muted-foreground">
                {{ deckSyncPlatformLabel(deck.platform) }}
              </Badge>
            </span>
            <span class="truncate text-xs text-muted-foreground">
              {{ deck.formatLabel }}
              <template v-if="deck.archetype"> · {{ deck.archetype }}</template>
              <template v-if="deck.lastPlayedAtMs !== null"> · {{ relativeTime(deck.lastPlayedAtMs, now) }}</template>
            </span>
          </div>
          <div class="flex shrink-0 items-center gap-2">
            <Spinner v-if="savingId === deck.id" class="size-3.5 text-muted-foreground" />
            <Switch
              :model-value="isOn(deck)"
              :disabled="deckSyncBlockedReason(deck, slots, linked, isOn(deck)) !== null || savingId !== null"
              :title="deckSyncBlockedReason(deck, slots, linked, isOn(deck)) ?? undefined"
              :aria-label="`Sync ${deck.name}`"
              @update:model-value="(value: boolean) => toggle(deck, value)"
            />
          </div>
        </div>

        <p v-if="deck.freesAt && !isOn(deck)" class="text-xs text-warning">{{ slotHeldText(deck.freesAt) }}</p>
        <p v-if="error && lastToggledId === deck.id" class="text-xs text-destructive">{{ error }}</p>
      </div>
    </div>

    <p v-if="error && lastToggledId === null" class="text-xs text-destructive">{{ error }}</p>

    <p v-if="slots.limit !== null" class="text-xs text-muted-foreground">{{ freePlanText(slots.limit) }}</p>
  </section>
</template>
