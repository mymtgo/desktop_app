<script setup lang="ts">
import { ChevronDown, Download, PencilLine, Plus } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { commands, type RunCardView } from '@/bindings'
import LeagueRunActions from '@/components/leagues/LeagueRunActions.vue'
import LeagueRunHeader from '@/components/leagues/LeagueRunHeader.vue'
import RunMatchesTable from '@/components/leagues/RunMatchesTable.vue'
import RunSummaryStats from '@/components/leagues/RunSummaryStats.vue'
import SetByYouBadge from '@/components/leagues/SetByYouBadge.vue'
import LeagueScreenshot from '@/components/screenshots/LeagueScreenshot.vue'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useLeagueEdit } from '@/composables/useLeagueEdit'
import { loadLeagueCover, useOffscreenScreenshot } from '@/composables/useOffscreenScreenshot'
import { useToast } from '@/composables/useToast'
import { runCardHeader } from '@/lib/leagueRunHeader'
import { LEAGUE_ROUNDS, leagueSummaryText, takesManualMatch } from '@/lib/leagues'
import { screenshotFileName } from '@/lib/screenshots'

/**
 * One league run (0.x `LeagueCard`): the shared `LeagueRunHeader` (score,
 * deck and run details, result dots) with the menu and chevron; expanded,
 * the game stats, the matches, "Add matches..." for any run (league
 * self-management spec 5.3; `LeagueRunActions` holds the menu and its
 * dialogs), "Add manual match" on an Active run or a manual league with
 * fewer than five matches (on the Leagues page, which hosts the sheet), and
 * the notes. The first card and live runs start
 * expanded.
 *
 * `compact` (the home dashboard's League in progress) is the header alone:
 * no chevron, never expanded; a click opens the run's deck (a limited run's
 * event). A limited run shows its name (`QNW Draft`) and colours in place
 * of the deck and links to its event; its menu is the constructed one, but
 * it takes no manual match.
 */
const props = withDefaults(
  defineProps<{
    league: RunCardView
    hideDeckIdentity?: boolean
    defaultExpanded?: boolean
    /** "Now" for the relative times (the data's fetch time); the render time when left out. */
    now?: number
    /** Offer "Add manual match" (the page listens for `create-match`). */
    canCreateMatch?: boolean
    /** The header alone: no chevron, never expands (the home dashboard). */
    compact?: boolean
  }>(),
  { hideDeckIdentity: false, defaultExpanded: false, now: undefined, canCreateMatch: false, compact: false },
)

const emit = defineEmits<{ 'create-match': [run: RunCardView] }>()

const router = useRouter()
const { add: toast } = useToast()
const { saving: unlinking, run: runEdit } = useLeagueEdit()

const expanded = ref(!props.compact && (props.defaultExpanded || props.league.classification === 'LIVE'))

const nowMs = computed(() => props.now ?? Date.now())
const header = computed(() => runCardHeader(props.league, nowMs.value, props.hideDeckIdentity))

const isLimited = computed(() => props.league.limited)
const isManual = computed(() => props.league.manual)
const canAddMatch = computed(() => isManual.value && props.league.matches.length < LEAGUE_ROUNDS)
const canCreateManualMatch = computed(
  () => !isLimited.value && props.canCreateMatch && takesManualMatch(props.league),
)
const isEmptyManual = computed(() => isManual.value && props.league.matches.length === 0)

/** A limited run's event: its draft, or its deck for sealed (no picks to review), as the Limited index. */
const limitedLink = computed(() => (isLimited.value ? header.value.link : null))

const actions = ref<InstanceType<typeof LeagueRunActions> | null>(null)

function editNotes() {
  actions.value?.editNotes()
}

function addMatch() {
  actions.value?.addMatches()
}

async function unlinkMatch(token: string) {
  await runEdit('Could not remove the match from the league', () => commands.unlinkLeagueMatch(props.league.key, token))
}

async function copySummary() {
  try {
    await navigator.clipboard.writeText(leagueSummaryText(props.league, nowMs.value))
  } catch {
    toast({ type: 'error', title: 'Could not copy the league summary', message: 'The clipboard is not available.' })
  }
}

// "Copy screenshot of league" (0.x `handleScreenshot`): the cover art is
// read only now, as a `data:` URL.
const screenshot = useOffscreenScreenshot<{ cover: string | null }>()
const { shot, busy: capturing, saveOffered, saving } = screenshot
const shotTarget = screenshot.target

function copyScreenshot() {
  void screenshot.take(
    async () => ({ cover: await loadLeagueCover(props.league.deck?.id ?? null) }),
    screenshotFileName(isLimited.value ? props.league.name : props.league.deck?.name, 'league'),
  )
}

function openDeck() {
  if (props.league.deck) void router.push(`/decks/${props.league.deck.id}`)
}

/** The header's click: compact opens the run (its deck, a limited run's event); else it toggles. */
function toggle() {
  if (!props.compact) {
    expanded.value = !expanded.value
  } else if (limitedLink.value) {
    void router.push(limitedLink.value)
  } else {
    openDeck()
  }
}
</script>

<template>
  <Card class="gap-0 overflow-hidden p-0">
    <!-- A click anywhere on the header toggles (as 0.x); the keyboard toggle is the chevron button, so no button holds another. -->
    <LeagueRunHeader :header="header" :now="nowMs" class="cursor-pointer hover:bg-muted/40" @click="toggle">
      <template #title-extra>
        <SetByYouBadge :user-state="league.userState" />
      </template>
      <template #actions>
        <Button
          v-if="saveOffered"
          variant="outline"
          size="sm"
          class="h-7"
          :disabled="saving"
          @click.stop="screenshot.save"
        >
          <Download class="mr-1.5 size-3.5" />
          Save image
        </Button>
        <LeagueRunActions
          ref="actions"
          :run-key="league.key"
          :match-count="league.matches.length"
          :manual="isManual"
          :limited="isLimited"
          :user-state="league.userState"
          :auto-state="league.autoState"
          :notes="league.notes"
          :disabled="capturing"
          @screenshot="copyScreenshot"
          @copy-summary="copySummary"
        />
        <button
          v-if="!compact"
          type="button"
          class="flex size-7 items-center justify-center rounded-md text-muted-foreground hover:text-foreground"
          :aria-expanded="expanded"
          :aria-label="expanded ? 'Hide league details' : 'Show league details'"
        >
          <ChevronDown class="size-4 transition-transform" :class="{ 'rotate-180': expanded }" />
        </button>
      </template>
    </LeagueRunHeader>

    <div v-if="expanded && !compact" class="flex flex-col gap-4 border-t border-border p-4">
      <RunSummaryStats
        :game-wins="league.gameWins"
        :game-losses="league.gameLosses"
        :on-play-record="league.onPlayRecord"
        :on-draw-record="league.onDrawRecord"
      />

      <RunMatchesTable
        v-if="!isEmptyManual"
        :matches="league.matches"
        can-unlink
        :busy="unlinking"
        @unlink="unlinkMatch"
      />

      <div
        v-else
        class="flex flex-col items-center gap-2 rounded-md border border-dashed border-border bg-card/50 p-8 text-center"
      >
        <p class="text-sm font-medium">No matches yet</p>
        <p class="text-xs text-muted-foreground">Pick existing matches played with this deck to build the league.</p>
      </div>

      <div v-if="canAddMatch || canCreateManualMatch" class="flex gap-2">
        <button
          v-if="canAddMatch"
          type="button"
          class="flex flex-1 items-center justify-center gap-2 rounded-md border border-dashed border-border p-3 text-sm text-muted-foreground hover:bg-muted/40 hover:text-foreground"
          @click="addMatch"
        >
          <Plus class="size-4" />
          Add match
        </button>
        <button
          v-if="canCreateManualMatch"
          type="button"
          class="flex flex-1 items-center justify-center gap-2 rounded-md border border-dashed border-border p-3 text-sm text-muted-foreground hover:bg-muted/40 hover:text-foreground"
          @click="emit('create-match', league)"
        >
          <PencilLine class="size-4" />
          Add manual match
        </button>
      </div>

      <section>
        <div class="mb-2 flex items-center gap-2">
          <h4 class="text-[10px] font-medium tracking-widest text-muted-foreground uppercase">Notes</h4>
          <Button variant="ghost" size="icon" class="size-5" aria-label="Edit notes" @click="editNotes">
            <PencilLine class="size-3" />
          </Button>
        </div>
        <p v-if="league.notes" class="text-sm whitespace-pre-wrap text-foreground">{{ league.notes }}</p>
        <button
          v-else
          type="button"
          class="w-full rounded-md border border-dashed border-border p-3 text-left text-sm text-muted-foreground italic hover:bg-muted/40"
          @click="editNotes"
        >
          Click to add notes for this league…
        </button>
      </section>
    </div>
  </Card>

  <div v-if="shot" style="position: fixed; top: -9999px; left: -9999px; pointer-events: none">
    <LeagueScreenshot ref="shotTarget" :league="league" :cover-art-base64="shot.cover" />
  </div>
</template>
