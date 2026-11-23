<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { Eye, PencilLine } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { ArchetypeCardView, CommanderCardView, GameDetailView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import EditRevealsDialog from '@/components/matches/EditRevealsDialog.vue'
import { Button } from '@/components/ui/button'
import {
  aggregateReveals,
  groupReveals,
  previewTop,
  revealBorderStyle,
  revealFilters,
  type AggregatedReveal,
  type RevealFilter,
} from '@/lib/matchDetail'

/**
 * The cards the opponent showed (0.x `OpponentRevealsRail.vue`): every game
 * or one (G-filters), grouped by type, a colour border per card and a card
 * preview on hover. A manual match edits the shown game's cards (game 1
 * when showing all).
 */
const props = withDefaults(
  defineProps<{
    games: GameDetailView[]
    /** A manual match: show the per-game edit button. */
    editable?: boolean
    token?: string
    /** The opponent archetype's list: the reveals dialog's quick picks. */
    archetypeDecklist?: ArchetypeCardView[] | null
    /** The opponent's commanders: shown in their own panel, not listed here. */
    commanders?: CommanderCardView[]
  }>(),
  { editable: false, token: '', archetypeDecklist: null, commanders: () => [] },
)

const revealsDialog = ref<InstanceType<typeof EditRevealsDialog> | null>(null)

const filter = ref<RevealFilter>('all')
const hovered = ref<AggregatedReveal | null>(null)
const top = ref(0)

const filters = computed(() => revealFilters(props.games))
const aggregated = computed(() => aggregateReveals(props.games, filter.value, props.commanders))
const groups = computed(() => groupReveals(aggregated.value))

/** The game an edit applies to: the filtered game, or game 1 when showing all. */
const editingGame = computed<GameDetailView | null>(() => {
  if (filter.value === 'all') return props.games[0] ?? null
  return props.games.find((g) => g.number === filter.value) ?? null
})

function filterLabel(value: RevealFilter): string {
  return value === 'all' ? 'All' : `G${value}`
}

function onCardEnter(card: AggregatedReveal, event: MouseEvent) {
  if (!card.image) return
  hovered.value = card
  const rowTop = (event.currentTarget as HTMLElement).getBoundingClientRect().top
  top.value = previewTop(rowTop, window.innerHeight)
}

function onCardLeave() {
  hovered.value = null
}
</script>

<template>
  <aside
    class="flex flex-col overflow-hidden rounded-md border border-white/5 bg-black/40 text-card-foreground shadow outline-1 outline-black/60"
  >
    <header class="panel-header justify-between gap-2">
      <span class="inline-flex items-center gap-1.5 text-[11px] font-semibold tracking-widest uppercase">
        <Eye :size="13" class="text-muted-foreground" />
        Opponent reveals
      </span>
      <div class="flex items-center gap-1">
        <Button
          v-if="editable && editingGame"
          variant="ghost"
          size="sm"
          class="h-6 px-1.5 text-[11px]"
          @click="revealsDialog?.open()"
        >
          <PencilLine :size="11" />
          {{ editingGame.opponentCardsSeen.length ? `Edit G${editingGame.number}` : `Add G${editingGame.number}` }}
        </Button>
        <span class="rounded-full bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground tabular-nums">
          {{ aggregated.length }}
        </span>
      </div>
    </header>

    <div v-if="filters.length" class="divider-bevel flex gap-1.5 px-3 py-2">
      <button
        v-for="value in filters"
        :key="value"
        type="button"
        class="toggle-tile cursor-pointer px-2 py-0.5 font-mono text-[10px] tracking-widest uppercase"
        :aria-pressed="filter === value"
        @click="filter = value"
      >
        {{ filterLabel(value) }}
      </button>
    </div>

    <div class="flex max-h-[60vh] flex-col gap-3 overflow-auto px-3 py-3">
      <section v-for="group in groups" :key="`group_${group.type}`">
        <h3 class="mb-0.5 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase">
          {{ group.type }} ({{ group.count }})
        </h3>
        <div
          v-for="card in group.cards"
          :key="card.name"
          :style="revealBorderStyle(card.colors)"
          class="flex items-center justify-between gap-2 py-0.5 pr-1 pl-2 text-xs"
          @mouseenter="onCardEnter(card, $event)"
          @mouseleave="onCardLeave"
        >
          <span class="min-w-0 truncate">
            <span class="font-semibold tabular-nums">{{ card.quantity }}</span>
            {{ card.name }}
          </span>
          <div class="flex shrink-0 items-center gap-1">
            <ManaColors v-if="card.colors.length" :colors="card.colors" size="xs" />
            <span
              v-for="g in card.games"
              :key="g"
              class="rounded bg-muted px-1 py-px font-mono text-[9px] text-muted-foreground"
            >
              G{{ g }}
            </span>
          </div>
        </div>
      </section>

      <div
        v-if="aggregated.length === 0"
        class="flex flex-col items-center gap-2 px-3 py-6 text-center text-xs text-muted-foreground italic"
      >
        <span>{{ editable ? 'No revealed cards recorded.' : 'No cards revealed in this game.' }}</span>
      </div>
    </div>

    <EditRevealsDialog v-if="editable && editingGame" ref="revealsDialog" :token="token" :game="editingGame" :games="games" :archetype-decklist="archetypeDecklist" />

    <Transition name="fade">
      <div v-if="hovered?.image" class="pointer-events-none fixed right-2 z-50" :style="{ top: `${top}px` }">
        <CardImage :src="hovered.image" :local="hovered.localImage" :alt="hovered.name" class="w-[200px] rounded-lg shadow-xl ring-1 ring-border" />
      </div>
    </Transition>
  </aside>
</template>

<style scoped>
.fade-enter-active {
  transition: opacity 0.1s ease;
}
.fade-leave-active {
  transition: opacity 0.05s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
