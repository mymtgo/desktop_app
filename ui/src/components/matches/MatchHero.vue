<script setup lang="ts">
import { Clock, Pencil, PencilLine, Plus, RefreshCw, TrendingDown, TrendingUp } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import type { MatchDetail } from '@/bindings'
import ArchetypeName from '@/components/archetypes/ArchetypeName.vue'
import SetByYouBadge from '@/components/leagues/SetByYouBadge.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import SetArchetypeDialog from '@/components/matches/SetArchetypeDialog.vue'
import { Button } from '@/components/ui/button'
import { createArchetypeRoute } from '@/lib/archetypes'
import { lastPlayedText } from '@/lib/decks'
import { gamesCountText, heroResult, ratingText, matchTimeText, scoreText, startedAtText } from '@/lib/matchDetail'
import { endstepRatingCell } from '@/lib/matchList'

/**
 * The match header (0.x `MatchHero.vue`): result, score, deck vs the
 * opponent's archetype (or "Unknown archetype") with the pencil that sets it
 * by hand, and the meta line. "Detect" shows where the page wires detection
 * (`detectable`, emits `detect`; it spins while `detecting`, as 0.x's
 * spinner did); the same pages offer "Create archetype
 * from this match" (the create form prefilled from the opponent's cards).
 * "Edit match" emits `edit`; "Set by you" marks a match the player fixed.
 */
const props = withDefaults(
  defineProps<{
    match: MatchDetail
    detectable?: boolean
    detecting?: boolean
  }>(),
  { detectable: false, detecting: false },
)

const emit = defineEmits<{ detect: []; edit: [] }>()

const router = useRouter()

function createArchetype() {
  void router.push(createArchetypeRoute({ sourceMatchId: props.match.id }))
}

const archetypeDialog = ref<InstanceType<typeof SetArchetypeDialog> | null>(null)

function editArchetype() {
  archetypeDialog.value?.openForMatch(
    props.match.token,
    props.match.format,
    props.match.opponentArchetype?.id ?? null,
  )
}

const result = computed(() => heroResult(props.match))
const totalGames = computed(() => props.match.wins + props.match.losses)
const since = computed(() => lastPlayedText(props.match.startedAtMs, Date.now()))
const matchTime = computed(() => matchTimeText(props.match.startedAtMs, props.match.endedAtMs))
</script>

<template>
  <SetArchetypeDialog ref="archetypeDialog" />
  <header class="divider-bevel -mx-4 flex flex-col gap-2.5 px-4 pb-4 lg:-mx-6 lg:px-6">
    <div class="flex flex-wrap items-center gap-x-3 gap-y-2">
      <!-- Score -->
      <span
        class="inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[10px] font-semibold tracking-widest uppercase"
        :class="
          result.win
            ? 'border-success/40 bg-success/10 text-success'
            : 'border-destructive/40 bg-destructive/10 text-destructive'
        "
      >
        <span class="size-1.5 rounded-full" :class="result.win ? 'bg-success' : 'bg-destructive'" />
        {{ result.text }}
      </span>
      <span class="font-mono text-base font-semibold tabular-nums">{{ scoreText(match.wins, match.losses) }}</span>
      <SetByYouBadge v-if="match.corrected" tooltip="Result set by you" />

      <span class="h-4 w-px bg-border" />

      <!-- Matchup -->
      <h1 class="flex flex-wrap items-center gap-1.5 text-sm font-semibold">
        <template v-if="match.deck">
          <ManaColors v-if="match.deck.colors.length" :colors="match.deck.colors" size="xs" />
          {{ match.deck.name }}
          <span class="font-normal text-muted-foreground/60">vs</span>
        </template>
        <template v-if="match.opponentArchetype">
          <ManaColors
            v-if="match.opponentArchetype.colors.length"
            :colors="match.opponentArchetype.colors"
            size="xs"
          />
          <ArchetypeName :archetype="match.opponentArchetype" />
        </template>
        <span
          v-else-if="match.archetypePending"
          class="text-muted-foreground italic"
          title="Detection runs in the background and tries again when MyMTGO can be reached."
          >Not detected yet</span
        >
        <span v-else class="text-muted-foreground italic">Unknown archetype</span>
        <button
          v-if="match.opponent"
          type="button"
          class="rounded p-0.5 text-muted-foreground hover:text-foreground"
          aria-label="Set archetype"
          title="Set archetype"
          @click="editArchetype"
        >
          <Pencil :size="12" />
        </button>
        <button
          v-if="detectable"
          type="button"
          class="rounded p-0.5 text-muted-foreground hover:text-foreground"
          aria-label="Detect"
          :title="detecting ? 'Detecting...' : 'Detect'"
          :disabled="detecting"
          @click="emit('detect')"
        >
          <RefreshCw :size="12" :class="{ 'animate-spin': detecting }" />
        </button>
        <button
          v-if="detectable && match.opponent"
          type="button"
          class="rounded p-0.5 text-muted-foreground hover:text-foreground"
          aria-label="Create archetype from this match"
          title="Create archetype from this match"
          @click="createArchetype"
        >
          <Plus :size="12" />
        </button>
        <span class="font-normal text-muted-foreground/60">&middot;</span>
        <span class="font-normal text-muted-foreground">{{ match.opponent ?? 'Unknown' }}</span>
      </h1>

      <Button variant="secondary" size="sm" class="ml-auto" @click="emit('edit')">
        <PencilLine :size="12" />
        Edit match
      </Button>
    </div>

    <div class="flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-[11px] text-muted-foreground">
      <template v-if="match.format">
        <span>{{ match.format }}</span>
        <span class="text-muted-foreground/40">&middot;</span>
      </template>
      <span>{{ startedAtText(match.startedAtMs) }}</span>
      <span class="text-muted-foreground/40">&middot;</span>
      <span>{{ since }}</span>
      <template v-if="matchTime">
        <span class="text-muted-foreground/40">&middot;</span>
        <span class="inline-flex items-center gap-1">
          <Clock :size="11" />
          {{ matchTime }}
        </span>
      </template>
      <span class="text-muted-foreground/40">&middot;</span>
      <span>{{ gamesCountText(totalGames) }}</span>
      <template v-if="match.leagueName">
        <span class="text-muted-foreground/40">&middot;</span>
        <span>{{ match.leagueName }}</span>
      </template>
      <template v-if="match.rating">
        <span class="text-muted-foreground/40">&middot;</span>
        <span data-testid="rating" class="inline-flex items-center gap-2">
          <span>{{ ratingText(match.rating) }}</span>
          <span class="inline-flex items-center gap-0.5" :class="endstepRatingCell(match.rating).tone">
            <TrendingUp v-if="endstepRatingCell(match.rating).trend === 'up'" class="size-3.5" aria-hidden="true" />
            <TrendingDown v-else-if="endstepRatingCell(match.rating).trend === 'down'" class="size-3.5" aria-hidden="true" />
            {{ endstepRatingCell(match.rating).change }}
          </span>
        </span>
      </template>
    </div>
  </header>
</template>
