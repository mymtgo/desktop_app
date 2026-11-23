<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import {
  Award,
  BarChart3,
  BookOpen,
  Boxes,
  LayoutDashboard,
  List,
  ScrollText,
  SettingsIcon,
  Swords,
  Trophy as TrophyIcon,
} from 'lucide-vue-next'
import { computed, type Component } from 'vue'
import { RouterLink } from 'vue-router'
import type { BestOfView, DeckHeaderView } from '@/bindings'
import BestOfSwitch from '@/components/decks/BestOfSwitch.vue'
import EditableDeckName from '@/components/decks/EditableDeckName.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { usePlatformContext } from '@/composables/usePlatformContext'
import type { Feature } from '@/lib/platformFeatures'
import { deckLinkQuery, showVersionSelect, shownVersion, versionOptionLabel } from '@/lib/deckRoute'
import { deckCover } from '@/lib/decks'

const props = defineProps<{
  header: DeckHeaderView
  /** The selected version (null: all versions). */
  version: number | null
  timeframe: string
  /** The route name of the page on screen. */
  currentPage: string
}>()

const emit = defineEmits<{
  'update:version': [version: number | null]
}>()

const cover = computed(() => deckCover(props.header.deck))

/** The select's value: a version id, or `__all__` for all versions. */
const ALL = '__all__'
const versionKey = (id: number | null) => (id === null ? ALL : String(id))

// Shows the version the page displays: on Decklist and Settings, the current
// one when none is chosen.
const selectedVersion = computed({
  get: () => versionKey(shownVersion(props.version, props.header.currentVersionId, props.currentPage)),
  set: (value: string) => emit('update:version', value === ALL ? null : Number(value)),
})

const { hasFeature } = usePlatformContext()
const { bestOf, showSwitch, bo1Mode, choose } = useDeckBestOfContext()

/** Bo1 has no game logs to read: these pages have no data there. */
const BO1_HIDDEN = ['deck-game-stats', 'deck-sideboard-guides']

/**
 * The deck's pages. The version follows every link; the timeframe only the
 * pages that use one, as 0.x did. Arena has no data for some (feature map).
 */
const navItems = computed(() => {
  const state = { version: props.version, timeframe: props.timeframe }
  const link = (name: string, withTimeframe: boolean) => ({
    name,
    params: { id: props.header.deck.id },
    query: deckLinkQuery(state, withTimeframe),
  })
  const items: { key: string; label: string; icon: Component; to: ReturnType<typeof link>; feature?: Feature }[] = [
    { key: 'deck-dashboard', label: 'Dashboard', icon: LayoutDashboard, to: link('deck-dashboard', true) },
    { key: 'deck-card-stats', label: 'Card Stats', icon: BarChart3, to: link('deck-card-stats', true), feature: 'cardStats' },
    { key: 'deck-game-stats', label: 'Game Stats', icon: Boxes, to: link('deck-game-stats', true), feature: 'gameStats' },
    { key: 'deck-matches', label: 'Matches', icon: Swords, to: link('deck-matches', true) },
    { key: 'deck-leagues', label: 'Leagues', icon: TrophyIcon, to: link('deck-leagues', true), feature: 'leagues' },
    { key: 'deck-tournaments', label: 'Tournaments', icon: Award, to: link('deck-tournaments', true), feature: 'tournaments' },
    { key: 'deck-matchups', label: 'Matchups', icon: ScrollText, to: link('deck-matchups', true) },
    { key: 'deck-sideboard-guides', label: 'Sideboard Guides', icon: BookOpen, to: link('deck-sideboard-guides', false), feature: 'sideboardGuides' },
    { key: 'deck-decklist', label: 'Decklist', icon: List, to: link('deck-decklist', false) },
    { key: 'deck-settings', label: 'Settings', icon: SettingsIcon, to: link('deck-settings', false) },
  ]
  return items
    .filter((item) => item.feature === undefined || hasFeature(item.feature))
    .filter((item) => !(bo1Mode.value && BO1_HIDDEN.includes(item.key)))
})
</script>

<template>
  <div class="flex h-full flex-col border-r border-black/80 bg-muted/20">
    <!-- Deck header -->
    <div class="relative h-24 overflow-hidden border-b border-black/60">
      <CardImage
        v-if="cover.hasCover"
        :src="cover.src"
        :local="cover.local"
        :custom="cover.custom"
        :alt="header.deck.name"
        class="pointer-events-none absolute inset-0 h-full w-full object-cover object-top opacity-50"
      />
      <div
        class="relative flex h-full flex-col items-start justify-center gap-1.5 px-4"
        :class="cover.hasCover ? '[text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]' : ''"
      >
        <!-- No identity shows no symbol here, as 0.x (not the colourless one). -->
        <ManaColors v-if="header.deck.colors.length > 0" :colors="header.deck.colors" class="shrink-0" />
        <EditableDeckName
          :key="header.deck.id"
          :deck-id="header.deck.id"
          :name="header.deck.name"
          :original-name="header.deck.originalName"
          :archived="header.deck.archived"
          class="w-full"
        />
      </div>
    </div>

    <!-- Deck meta + version selector -->
    <div class="flex flex-col gap-2 border-b border-black/80 px-3 py-3">
      <div class="flex items-center gap-2">
        <span class="rounded-md border border-black/70 bg-background px-2.5 py-1 text-xs font-medium">
          {{ header.deck.formatLabel }}
        </span>
        <BestOfSwitch
          v-if="showSwitch && bestOf !== null"
          :model-value="bestOf"
          @update:model-value="(kind: BestOfView) => choose(kind)"
        />
        <span
          v-else-if="header.platform === 'mtgo' && header.trophies !== null"
          class="flex items-center gap-1 rounded-md border border-black/70 bg-background px-2.5 py-1 text-xs font-medium text-yellow-400"
          aria-label="Trophies"
        >
          <TrophyIcon class="size-3" />
          {{ header.trophies }}
        </span>
      </div>
      <Select v-if="showVersionSelect(header.versions)" v-model="selectedVersion">
        <SelectTrigger class="h-8 w-full text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="option in header.versions"
            :key="versionKey(option.id)"
            :value="versionKey(option.id)"
            :text-value="versionOptionLabel(option)"
          >
            {{ option.label }}
            <span v-if="option.isCurrent" class="ml-1 text-muted-foreground">&middot; Current</span>
            <span v-if="option.dateLabel" class="ml-1 text-muted-foreground">&middot; {{ option.dateLabel }}</span>
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <!-- Navigation -->
    <nav class="flex flex-1 flex-col gap-0.5 border-t border-white/5 px-2 py-3">
      <RouterLink
        v-for="item in navItems"
        :key="item.key"
        :to="item.to"
        class="flex items-center gap-3 rounded px-3 py-2 text-sm font-medium transition-colors"
        :class="currentPage === item.key ? 'nav-item-active' : 'nav-item-inactive'"
      >
        <component
          :is="item.icon"
          class="size-4 shrink-0 transition-[color,filter] duration-150"
          :class="{ 'nav-icon-active': currentPage === item.key }"
        />
        <span>{{ item.label }}</span>
      </RouterLink>
    </nav>
  </div>
</template>
