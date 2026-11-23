<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Download, ImageOff, Layers, Search } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { commands, type CardTileView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import CardTypeFilter from '@/components/cards/CardTypeFilter.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import PagedNav from '@/components/PagedNav.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { Switch } from '@/components/ui/switch'
import {
  POPULATED_TOAST,
  archetypeCountLabel,
  cardsFromUrl,
  cardsUrlQuery,
  hiddenTypesFrom,
  popularityTitle,
  typeToggles,
  type CardsState,
} from '@/lib/cards'
import { TYPE_KEYS, type FilterKey } from '@/lib/cardStats/typeFilter'
import { keys, useCardArchetypes, useCardsPage } from '@/lib/queries'
import { useDebouncedSearch } from '@/composables/useDebouncedSearch'
import { useToast } from '@/composables/useToast'

// 0.x Cards page: search, format, type and grouping filters, a grid of
// cards by popularity (the held archetypes holding each card), and a sheet
// of the archetypes holding a card.
// The filters live in the URL, so back and forward step through them.
const ALL = '__all__'
// The eight types: this page has no Sideboard toggle.
const typeKeys: FilterKey[] = [...TYPE_KEYS]
const route = useRoute()
const router = useRouter()
const queryClient = useQueryClient()
const { add: toast } = useToast()

const state = computed<CardsState>(() => cardsFromUrl(route.query))
const { data, error, isPending } = useCardsPage(state)

function setState(next: CardsState) {
  void router.replace({ query: cardsUrlQuery(next) })
}

// Every filter change goes back to the first page.
function change(next: Partial<CardsState>) {
  setState({ ...state.value, ...next, page: 1 })
}

const search = useDebouncedSearch({
  current: () => state.value.search,
  apply: (next) => change({ search: next }),
})

const populating = ref(false)
async function fetchMissing() {
  populating.value = true
  try {
    const answer = await commands.fetchMissingCards()
    if (answer.status === 'ok') {
      toast({ type: 'success', ...POPULATED_TOAST, duration: 2000 })
    } else {
      toast({ type: 'error', title: 'Fetch failed', message: answer.error.message })
    }
  } finally {
    populating.value = false
    void queryClient.invalidateQueries({ queryKey: keys.allCardsPage })
    void queryClient.invalidateQueries({ queryKey: keys.cardData })
  }
}

const sheetOpen = ref(false)
const selected = ref<CardTileView | null>(null)
const group = computed(() => (sheetOpen.value ? (selected.value?.group ?? null) : null))
const sheetFormat = computed(() => state.value.format)
const { data: archetypes, isPending: archetypesLoading } = useCardArchetypes(group, sheetFormat)

function openCard(card: CardTileView) {
  selected.value = card
  sheetOpen.value = true
}

function colours(identity: string | null): string[] {
  return identity ? identity.split(',') : []
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex flex-col gap-1">
      <h1 class="text-base font-semibold tracking-tight">Cards</h1>
      <p class="text-sm text-muted-foreground">
        Every card in the metagame's archetype lists, ranked by how many archetypes play it. Select a card to see which archetypes play it.
      </p>
    </div>

    <div class="flex flex-wrap items-center gap-3">
      <div class="relative w-64">
        <Search class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input v-model="search" placeholder="Search cards..." aria-label="Search cards" class="pl-8" />
      </div>

      <Select
        :model-value="state.format ?? ALL"
        @update:model-value="(value) => change({ format: value === ALL ? null : String(value) })"
      >
        <SelectTrigger class="w-40">
          <SelectValue placeholder="All formats" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem :value="ALL">All formats</SelectItem>
          <SelectItem v-for="format in data?.formats ?? []" :key="format.key" :value="format.key">
            {{ format.label }}
          </SelectItem>
        </SelectContent>
      </Select>

      <CardTypeFilter
        :model-value="typeToggles(state.hiddenTypes)"
        :keys="typeKeys"
        @update:model-value="(toggles) => change({ hiddenTypes: hiddenTypesFrom(toggles) })"
      />

      <label class="flex items-center gap-2 text-sm text-muted-foreground">
        <Switch :model-value="state.groupPrintings" @update:model-value="(value) => change({ groupPrintings: !!value })" />
        Group printings
      </label>

      <div v-if="data" class="ml-auto flex items-center gap-3">
        <span class="text-xs text-muted-foreground">
          {{ data.totalCount }} cards
          <template v-if="data.missingCount > 0">
            &middot; <span class="text-amber-400">{{ data.missingCount }} missing data</span>
          </template>
        </span>
        <Button v-if="data.missingCount > 0" size="sm" class="h-8" :disabled="populating" @click="fetchMissing">
          <Download class="mr-1.5 size-3.5" :class="{ 'animate-bounce': populating }" />
          Fetch missing ({{ data.missingCount }})
        </Button>
      </div>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load cards: {{ error.message }}</p>

    <div v-else-if="isPending || !data" class="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-8">
      <Skeleton v-for="n in 16" :key="n" class="aspect-5/7 w-full" />
    </div>

    <div
      v-else-if="data.cards.length"
      class="grid grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-8"
    >
      <Card v-for="card in data.cards" :key="card.catalogId" class="gap-0 overflow-hidden py-0 transition-colors hover:border-primary/50">
        <CardContent class="p-0">
          <button type="button" class="group relative block w-full cursor-pointer text-left" @click="openCard(card)">
            <div class="aspect-5/7 w-full bg-muted">
              <CardImage
                v-if="card.image || card.localImage"
                :src="card.image"
                :local="card.localImage"
                :alt="card.name ?? 'Unknown card'"
                loading="lazy"
                class="h-full w-full rounded-[10px] object-cover"
              />
              <div v-else class="flex h-full w-full flex-col items-center justify-center gap-1 text-muted-foreground">
                <ImageOff class="size-6" />
                <span class="px-2 text-center text-xs">{{ card.name ?? 'Unknown' }}</span>
              </div>
            </div>
            <Badge
              v-if="card.popularity > 0"
              class="absolute top-1.5 right-1.5 gap-1 bg-background/85 text-foreground backdrop-blur"
              variant="secondary"
              :title="popularityTitle(card.popularity)"
            >
              <Layers class="size-3" />
              {{ card.popularity }}
            </Badge>
          </button>
          <div class="flex items-center justify-between gap-2 px-2 py-1.5">
            <span class="truncate text-xs font-medium" :title="card.name ?? ''">{{ card.name ?? 'Unknown' }}</span>
            <span v-if="card.setCode" class="shrink-0 font-mono text-[10px] text-muted-foreground uppercase">
              {{ card.setCode }}
            </span>
          </div>
        </CardContent>
      </Card>
    </div>

    <div v-else class="flex flex-col items-center justify-center gap-2 py-20 text-muted-foreground">
      <Search class="size-8" />
      <p class="text-sm">No cards match your filters.</p>
    </div>

    <PagedNav
      v-if="data && data.lastPage > 1"
      class="justify-center"
      :per-page="data.perPage"
      :total="data.total"
      :page="data.page"
      @update:page="(page) => setState({ ...state, page })"
    />

    <Sheet v-model:open="sheetOpen">
      <SheetContent class="flex w-full flex-col gap-0 overflow-y-auto sm:max-w-md">
        <SheetHeader>
          <SheetTitle class="flex items-center gap-3">
            <CardImage
              v-if="selected && (selected.image || selected.localImage)"
              :src="selected.image"
              :local="selected.localImage"
              :alt="selected.name ?? ''"
              class="h-16 w-auto rounded-md"
            />
            <span>{{ selected?.name ?? 'Card' }}</span>
          </SheetTitle>
        </SheetHeader>

        <div class="flex flex-col gap-2 px-4 pb-4">
          <div v-if="archetypesLoading" class="flex flex-col gap-2">
            <Skeleton v-for="n in 4" :key="n" class="h-12 rounded-lg" />
          </div>
          <p v-else-if="!archetypes?.length" class="py-8 text-center text-sm text-muted-foreground">
            This card hasn't appeared in any tracked archetypes yet.
          </p>
          <template v-else>
            <p class="pb-1 text-xs text-muted-foreground">{{ archetypeCountLabel(archetypes.length) }}</p>
            <div
              v-for="archetype in archetypes"
              :key="archetype.id"
              class="flex items-center justify-between gap-3 rounded-lg border border-border bg-card px-3 py-2"
            >
              <div class="flex min-w-0 flex-col gap-1">
                <div class="flex items-center gap-2">
                  <ManaColors :colors="colours(archetype.colorIdentity)" class="shrink-0" />
                  <span class="truncate text-sm font-medium">{{ archetype.name }}</span>
                </div>
                <span v-if="archetype.formatLabel" class="text-[11px] text-muted-foreground">
                  {{ archetype.formatLabel }} &middot; {{ archetype.deckCount }} deck{{ archetype.deckCount === 1 ? '' : 's' }}
                </span>
              </div>
              <div class="flex shrink-0 gap-1">
                <Badge v-if="archetype.maindeck" variant="secondary">Main</Badge>
                <Badge v-if="archetype.sideboard" variant="outline">Side</Badge>
              </div>
            </div>
          </template>
        </div>
      </SheetContent>
    </Sheet>
  </div>
</template>
