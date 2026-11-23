<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { BookOpen, Calendar, Layers2, List, Swords } from 'lucide-vue-next'
import { computed } from 'vue'
import type { LimitedEventView, LimitedPage } from '@/bindings'
import StateBadge from '@/components/limited/StateBadge.vue'
import { limitedPath, recordTone } from '@/lib/limited'

/**
 * The event's sidebar (0.x `LimitedSidebar`): cover art, title and set, the
 * set, kind and record chips, the start, state and picks or packs, and the
 * pages (the command leaves Draft out for sealed).
 */
const props = defineProps<{ event: LimitedEventView; currentPage: string }>()

const icons: Record<LimitedPage, typeof BookOpen> = { draft: BookOpen, deck: Layers2, matches: Swords, cards: List }

const navItems = computed(() =>
  props.event.nav.map((item) => ({
    key: item.page,
    label: item.label,
    icon: icons[item.page],
    to: limitedPath(props.event.key, item.page),
  })),
)

const recordClass = computed(() => recordTone(props.event.wins, props.event.losses))
</script>

<template>
  <div class="flex h-full flex-col border-r border-black/80 bg-muted/20">
    <div class="relative h-24 overflow-hidden border-b border-black/60">
      <CardImage
        v-if="event.coverArt"
        :src="event.coverArt"
        :local="event.localCoverArt"
        :alt="event.title"
        class="pointer-events-none absolute inset-0 h-full w-full object-cover object-top opacity-50"
      />
      <div
        class="relative flex h-full flex-col items-start justify-center gap-1 px-4"
        :class="event.coverArt ? '[text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]' : ''"
      >
        <span class="text-sm leading-tight font-semibold">{{ event.title }}</span>
        <span class="text-xs text-muted-foreground">{{ event.subtitle }}</span>
      </div>
    </div>
    <div class="flex flex-col gap-2 border-b border-black/80 px-3 py-3">
      <div class="flex flex-wrap items-center gap-2">
        <span
          v-if="event.setLabel"
          class="rounded-md border border-black/70 bg-background px-2.5 py-1 text-xs font-medium"
        >
          {{ event.setLabel }}
        </span>
        <span class="rounded-md border border-black/70 bg-background px-2.5 py-1 text-xs font-medium capitalize">
          {{ event.kind }}
        </span>
        <span
          class="rounded-md border border-black/70 bg-background px-2.5 py-1 text-xs font-medium tabular-nums"
          :class="recordClass"
        >
          {{ event.wins }}-{{ event.losses }}
        </span>
      </div>
      <div v-if="event.startedAtHuman" class="inline-flex items-center gap-1 text-xs text-muted-foreground">
        <Calendar class="size-3" />{{ event.startedAtHuman }}
      </div>
      <div v-if="event.untrackedMatches > 0" class="text-xs text-muted-foreground tabular-nums">
        {{ event.untrackedMatches }} {{ event.untrackedMatches === 1 ? 'match' : 'matches' }} played elsewhere
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <StateBadge :label="event.state" :variant="event.stateVariant" />
        <span v-if="event.packs !== null" class="text-xs text-muted-foreground tabular-nums">
          {{ event.packs }} packs
        </span>
        <span v-else class="text-xs text-muted-foreground tabular-nums">
          {{ event.picksMade }}/{{ event.picksExpected }} picks
        </span>
      </div>
    </div>
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
