<script setup lang="ts">
import { ArrowRight, ChevronDown, Timer } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { commands, type LimitedIndexRowView, type PlatformView } from '@/bindings'
import LeagueRunActions from '@/components/leagues/LeagueRunActions.vue'
import LeagueRunHeader from '@/components/leagues/LeagueRunHeader.vue'
import RunMatchesTable from '@/components/leagues/RunMatchesTable.vue'
import RunSummaryStats from '@/components/leagues/RunSummaryStats.vue'
import SetByYouBadge from '@/components/leagues/SetByYouBadge.vue'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { useLeagueEdit } from '@/composables/useLeagueEdit'
import { limitedRowHeader } from '@/lib/leagueRunHeader'
import { limitedMatchPath, rowInfo, rowLinks, withOpponentColours } from '@/lib/limited'
import { useLimitedRunColours } from '@/lib/queries'

/**
 * One Limited event on the index (0.x `LimitedEventRow`): the Leagues
 * page's header (`LeagueRunHeader`: score, event, kind, state and result
 * dots, the info line) with the packs or picks, deck versions and pick time,
 * the opponents line, Deck and Review draft, and for a league run the same
 * "..." menu as the league card (`LeagueRunActions`, without the copy items:
 * Edit notes, Add matches, Set status, Delete; on Arena, Set status alone) and a "Set by you" badge when
 * the player set the status; expanded, the run's game stats and matches with the opponents' colours
 * (read only once the row is expanded: they need every match's events), with
 * a remove button on each match.
 */
const props = withDefaults(
  defineProps<{
    row: LimitedIndexRowView
    defaultExpanded?: boolean
    /** False on Arena: its runs take only a status, so the menu offers Set status alone and match removal hides. */
    editable?: boolean
    /** The run's game, for the Drop dialog's wording. */
    platform?: PlatformView
  }>(),
  { defaultExpanded: false, editable: true, platform: 'mtgo' },
)

/** Nothing to expand into until the run has played a match. */
const canExpand = computed(() => props.row.matches.length > 0)

/** As the league card: the newest run, and any live run, open on arrival. */
const expanded = ref(canExpand.value && (props.defaultExpanded || props.row.stateVariant === 'default'))

const runKey = computed(() => props.row.runKey)
const colours = useLimitedRunColours(runKey, expanded)
const matches = computed(() => withOpponentColours(props.row.matches, colours.data.value))

const { saving: unlinking, run: runEdit } = useLeagueEdit()

/** A league run has a key to manage; a tournament draft has no league row, an unlinked draft no run. */
const managedKey = computed(() => (props.row.linked && props.row.inLeague ? props.row.runKey : null))

async function unlinkMatch(token: string) {
  const key = managedKey.value
  if (key === null) return
  await runEdit('Could not remove the match from the league', () => commands.unlinkLeagueMatch(key, token))
}

const header = computed(() => limitedRowHeader(props.row))
const links = computed(() => rowLinks(props.row))
const info = computed(() => rowInfo(props.row))

/** A match row opens inside the event (0.x `limited.match`); an unlinked draft has no matches. */
const matchRoute = computed(() => {
  const key = props.row.runKey
  return key === null ? undefined : (token: string) => limitedMatchPath(key, token)
})
</script>

<template>
  <Card class="gap-0 overflow-hidden p-0">
    <LeagueRunHeader :header="header">
      <template #title-extra>
        <SetByYouBadge :user-state="row.userState" />
      </template>

      <template #info-extra>
        <span>{{ info.count }}</span>
        <span v-if="info.versions">{{ info.versions }}</span>
        <span v-if="info.avgPick" class="inline-flex items-center gap-1"><Timer class="size-3" />{{ info.avgPick }}</span>
        <span v-if="row.untrackedMatches > 0">
          {{ row.untrackedMatches }} {{ row.untrackedMatches === 1 ? 'match' : 'matches' }} played elsewhere
        </span>
      </template>

      <div v-if="info.opponents" class="text-xs text-muted-foreground">{{ info.opponents }}</div>
      <div v-else-if="info.note" class="text-xs text-muted-foreground/70 italic">{{ info.note }}</div>

      <template v-if="row.linked" #actions>
        <div class="flex items-center gap-2">
          <LeagueRunActions
            v-if="managedKey"
            :run-key="managedKey"
            :match-count="row.matches.length"
            :manual="false"
            limited
            :user-state="row.userState"
            :auto-state="row.autoState"
            :notes="row.notes"
            :shareable="false"
            :status-only="!editable"
            :platform="platform"
          />
          <Button v-if="links.deck" as-child variant="outline" size="sm">
            <RouterLink :to="links.deck">Deck</RouterLink>
          </Button>
          <Button v-if="links.draft" as-child size="sm">
            <RouterLink :to="links.draft">Review draft <ArrowRight class="size-3.5" /></RouterLink>
          </Button>
          <Button
            v-if="canExpand"
            variant="ghost"
            size="icon"
            class="size-8 text-muted-foreground"
            :aria-label="expanded ? 'Hide matches' : 'Show matches'"
            :aria-expanded="expanded"
            @click="expanded = !expanded"
          >
            <ChevronDown class="size-4 transition-transform" :class="{ 'rotate-180': expanded }" />
          </Button>
        </div>
      </template>
    </LeagueRunHeader>

    <div v-if="expanded && canExpand" class="flex flex-col gap-4 border-t border-border p-4">
      <RunSummaryStats
        :game-wins="row.gameWins"
        :game-losses="row.gameLosses"
        :on-play-record="row.onPlayRecord"
        :on-draw-record="row.onDrawRecord"
      />
      <RunMatchesTable
        :matches="matches"
        show-colours
        :match-route="matchRoute"
        :can-unlink="editable && managedKey !== null"
        :busy="unlinking"
        @unlink="unlinkMatch"
      />
    </div>
  </Card>
</template>
