<script setup lang="ts">
import { AlertTriangle, ChevronLeft, PencilLine } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { TabsList, TabsTrigger } from 'reka-ui'
import ManualMatchSheet from '@/components/matches/ManualMatchSheet.vue'
import MatchGame from '@/components/matches/MatchGame.vue'
import MatchHero from '@/components/matches/MatchHero.vue'
import MatchNotes from '@/components/matches/MatchNotes.vue'
import OpponentCommanders from '@/components/matches/OpponentCommanders.vue'
import OpponentRevealsRail from '@/components/matches/OpponentRevealsRail.vue'
import type { GameDetailView, MatchDetail } from '@/bindings'
import { hasFeature } from '@/lib/platformFeatures'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Tabs, TabsContent } from '@/components/ui/tabs'
import {
  durationText,
  gameTile,
  importedWithoutDetail,
  mulliganStrip,
  noDetailText,
  playDrawText,
  turnsText,
} from '@/lib/matchDetail'

/**
 * One match's detail (0.x `matches/partials/MatchDetail.vue`): the back link,
 * the manual or imported banner, the hero, the notes, the game tabs and the
 * opponent's reveals. The match page and the Limited match page both show it;
 * "Back to matches" steps back, or goes to `fallbackPath` without history.
 * "Edit match" opens the manual match sheet on this match (edit a match spec).
 */
const props = withDefaults(
  defineProps<{
    match: MatchDetail
    fallbackPath: string
    /** The hero offers "Detect" and "Create archetype from this match". */
    detectable?: boolean
    /** Detection is running on this match. */
    detecting?: boolean
  }>(),
  { detectable: false, detecting: false },
)

const emit = defineEmits<{ detect: [] }>()

const router = useRouter()

const editSheet = ref<InstanceType<typeof ManualMatchSheet> | null>(null)

const opponentName = computed(() => props.match.opponent ?? 'Opponent')

/** A game tile's badge colours: a result never observed is neutral. */
const TILE_CLASSES: Record<'win' | 'loss' | 'unknown', string> = {
  win: 'bg-success/15 text-success ring-1 ring-success/30',
  loss: 'bg-destructive/15 text-destructive ring-1 ring-destructive/30',
  unknown: 'bg-muted text-muted-foreground ring-1 ring-border',
}

// A platform without match detail shows each game's result and length only.
// MTGO, Arena and Endstep have it: an Arena or Endstep game's panels read its
// stored game rows, an Arena game's log its stored messages and an Endstep
// game's log its stored frames. An Arena or Endstep game's Replay opens its
// `.mtgr` record, built when it opens (`hasFrames`).
const hasDetail = computed(() => hasFeature(props.match.platform, 'matchDetail'))

/** A game tile's line: play or draw, length, turns and who mulliganed. */
function gameFacts(game: GameDetailView): string[] {
  if (!hasDetail.value) {
    const duration = durationText(game.startedAtMs, game.endedAtMs)
    return duration ? [duration] : []
  }
  const facts = [playDrawText(game.onThePlay)]
  const duration = durationText(game.startedAtMs, game.endedAtMs)
  if (duration) facts.push(duration)
  const turns = turnsText(game.turns)
  if (turns !== null) facts.push(`${turns} turns`)
  for (const cell of mulliganStrip(game, opponentName.value)) facts.push(`${cell.label} ${cell.value}`)
  return facts
}

// An imported match shows its games as a tracked one does once any game has
// 0.x detail; without any, the banner says what is missing.
const withoutDetail = computed(() => importedWithoutDetail(props.match))

// The open game tab: a match opens on its first game, and a refetch keeps
// the user's pick while that game is still there.
const activeGame = ref('1')
let tabsFor: string | null = null
watch(
  () => props.match,
  (detail) => {
    const stillThere = detail.games.some((g) => String(g.number) === activeGame.value)
    if (tabsFor !== detail.token || !stillThere) {
      activeGame.value = String(detail.games[0]?.number ?? 1)
      tabsFor = detail.token
    }
  },
  { immediate: true },
)

function goBack() {
  // Vue Router records the previous entry; without one there is nothing to step back to.
  if (window.history.state?.back) {
    router.back()
  } else {
    void router.push(props.fallbackPath)
  }
}
</script>

<template>
  <div class="flex flex-col gap-5 p-4 lg:p-6">
    <!-- Back link -->
    <Button variant="secondary" size="sm" class="self-start" @click="goBack">
      <ChevronLeft />
      Back to matches
    </Button>

    <!-- Manual / imported banner -->
    <Card v-if="match.manual" class="border-sky-500/30 bg-sky-500/5 py-0">
      <CardContent class="flex items-center gap-2 p-3 text-sm text-sky-600 dark:text-sky-400">
        <PencilLine class="size-4 shrink-0" />
        Manual match. Entered by hand, so there is no game log or replay. Hands, sideboard changes and revealed cards
        can be added per game.
      </CardContent>
    </Card>
    <Card v-else-if="withoutDetail" class="border-yellow-500/30 bg-yellow-500/5 py-0">
      <CardContent class="flex items-center gap-2 p-3 text-sm text-yellow-600 dark:text-yellow-400">
        <AlertTriangle class="size-4 shrink-0" />
        This is an imported match. Opening hands, sideboard changes, and turn estimates are not available.
      </CardContent>
    </Card>

    <!-- Hero -->
    <MatchHero :match="match" :detectable="detectable" :detecting="detecting" @detect="emit('detect')" @edit="editSheet?.openEdit(match.id)" />
    <ManualMatchSheet ref="editSheet" />

    <!-- Notes -->
    <MatchNotes :key="match.token" :token="match.token" :notes="match.notes" />

    <!-- Main grid -->
    <div class="grid grid-cols-1 gap-6 xl:grid-cols-[minmax(0,1fr)_320px]">
      <!-- Game tabs + panel -->
      <Tabs v-model="activeGame" class="flex flex-col gap-4">
        <TabsList class="grid gap-3" :style="{ gridTemplateColumns: `repeat(${match.games.length}, minmax(0, 1fr))` }">
          <TabsTrigger
            v-for="game in match.games"
            :key="game.number"
            :value="String(game.number)"
            class="toggle-tile flex cursor-pointer flex-col items-start gap-1 px-4 py-2.5 text-left outline-offset-0 focus-visible:ring-[3px] focus-visible:ring-ring/50"
          >
            <div class="flex items-center gap-2">
              <span class="font-mono text-[10px] tracking-widest uppercase">Game {{ game.number }}</span>
              <span
                class="rounded px-1.5 py-0.5 font-mono text-[9px] font-semibold tracking-widest uppercase"
                :class="TILE_CLASSES[gameTile(game).tone]"
              >
                {{ gameTile(game).text }}
              </span>
            </div>
            <span class="flex flex-wrap gap-x-1.5 text-xs">
              <span v-for="(fact, i) in gameFacts(game)" :key="i">
                <span v-if="i > 0" class="mr-1.5 text-muted-foreground/40">·</span>{{ fact }}
              </span>
            </span>
          </TabsTrigger>
        </TabsList>

        <TabsContent v-for="game in match.games" :key="game.number" :value="String(game.number)" class="mt-0">
          <p v-if="!hasDetail" class="text-sm text-muted-foreground">
            {{ noDetailText(match.platform) }}
          </p>
          <MatchGame
            v-else
            :token="match.token"
            :source="match.source"
            :game="game"
            :imported="withoutDetail"
            :manual="match.manual"
            :manual-editing="match.manualEditing"
          />
        </TabsContent>
      </Tabs>

      <!-- Right rail: a Brawl match's opponent commander above the reveals. -->
      <div v-if="hasDetail && !withoutDetail" class="flex flex-col gap-4 xl:sticky xl:top-4 xl:self-start">
        <OpponentCommanders v-if="match.opponentCommanders.length" :commanders="match.opponentCommanders" />
        <OpponentRevealsRail
          :games="match.games"
          :editable="match.manual && match.manualEditing !== null"
          :token="match.token"
          :archetype-decklist="match.manualEditing?.archetypeDecklist ?? null"
          :commanders="match.opponentCommanders"
        />
      </div>
    </div>
  </div>
</template>
