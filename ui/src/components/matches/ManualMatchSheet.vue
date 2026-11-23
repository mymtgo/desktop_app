<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { BestOfView, ManualDeckOption, PlatformView, RankGapKey } from '@/bindings'
import BestOfSwitch from '@/components/decks/BestOfSwitch.vue'
import ArchetypePicker from '@/components/matches/ArchetypePicker.vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { useMatchEdit } from '@/composables/useMatchEdit'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { useArchetypeOptions } from '@/composables/useOpponentArchetype'
import {
  canSubmitEdit,
  canSubmitMatch,
  correctionPayload,
  editFormFromView,
  emptyGames,
  endedAtFor,
  errorsBesides,
  gamesForKind,
  gamesPayload,
  isGameEnabled,
  keepLeague,
  leagueOptionsFor,
  MATCH_MINUTES,
  NO_RANK_GAP,
  plusMinutes,
  rankGapFor,
  rankGapValue,
  setResult as nextResults,
  toLocalInput,
  turnsError,
  type FieldErrors,
  type GameRow,
} from '@/lib/manualMatch'
import { useManualLeagueOptions, useManualRankGaps } from '@/lib/queries'

/**
 * "Add manual match" (0.x `ManualMatchSheet.vue`): a match the tracker did
 * not see, with the opponent's archetype (optional; the deck's format's
 * archetypes and the fallbacks).
 *
 * "Edit match" (`openEdit`, edit a match spec section 3): the same sheet on
 * any match, filled with its opponent, times and games. The deck, league and
 * archetype have their own controls, so they are hidden. The opponent is
 * needed as Add needs it; a blank end keeps a match with no known end. The
 * match's own platform (not the context's) decides: an Arena match's
 * opponent is its Arena name and optional (a blank one clears it).
 *
 * In the Arena context Add has no league, the opponent is optional, and the
 * match is Ranked or Unranked; a ranked one may name the rank gap it was
 * played in (a gap played elsewhere), so the deck's climb counts it.
 */
const props = withDefaults(
  defineProps<{
    /** The decks to pick from (the Leagues page). */
    decks?: ManualDeckOption[]
    /** Fix the deck and hide the deck select (a deck's Matches page). */
    deckId?: number
    /** The fixed deck's format code, for the archetype picker. */
    format?: string | null
  }>(),
  { decks: () => [], deckId: undefined, format: null },
)

const open = ref(false)
/** The match row being edited; `null` while adding. */
const editingId = ref<number | null>(null)
const editing = computed(() => editingId.value !== null)
/** The start and end the edit sheet opened with: Save sends only a changed one. */
const openedTimes = ref({ startedAt: '', endedAt: '' })
/** The Add sheet's lookups load only while it is open for adding. */
const adding = computed(() => open.value && !editing.value)
const deckId = ref<number | null>(props.deckId ?? null)
const opponentName = ref('')
const league = ref<string | null>(null)
const startedAt = ref(toLocalInput(new Date()))
const endedAt = ref(plusMinutes(startedAt.value, MATCH_MINUTES))
const endedAtTouched = ref(false)
const games = ref<GameRow[]>(emptyGames())
const errors = ref<FieldErrors>({})
const archetypeId = ref<number | null>(null)
const choosingArchetype = ref(false)
const ranked = ref<'ranked' | 'unranked'>('ranked')
const bestOf = ref<BestOfView>('bo3')
/** The platform of the match being edited. */
const editPlatform = ref<PlatformView>('mtgo')
const rankGap = ref<string>(NO_RANK_GAP)

const { platform } = usePlatformContext()
/** Adding follows the context; editing follows the match. */
const arena = computed(() => (editing.value ? editPlatform.value : platform.value) === 'arena')
const rankedOptions = [
  { value: 'ranked', label: 'Ranked' },
  { value: 'unranked', label: 'Unranked' },
]
const { data: gapData } = useManualRankGaps(
  deckId,
  computed(() => adding.value && arena.value),
)
const gapOptions = computed(() => gapData.value ?? [])
// A refused gap keeps the field (and its error) even when no gap is left to choose.
const showRankGap = computed(
  () =>
    !editing.value &&
    arena.value &&
    ranked.value === 'ranked' &&
    (gapOptions.value.length > 0 || errors.value.rank_gap !== undefined),
)

const deckFormat = computed(
  () => props.decks.find((deck) => deck.id === deckId.value)?.formatCode ?? props.format ?? null,
)
const { data: archetypeOptions } = useArchetypeOptions(deckFormat, adding)
const archetypeName = computed(
  () => archetypeOptions.value?.find((option) => option.id === archetypeId.value)?.name ?? null,
)

// Another deck may be another format: its archetypes are not this one's.
watch(deckFormat, () => {
  archetypeId.value = null
})

// Another deck's gaps are not this one's.
watch(deckId, () => {
  rankGap.value = NO_RANK_GAP
})

function chooseArchetype(id: number) {
  archetypeId.value = id
  choosingArchetype.value = false
}

const { saving, createManual, correctMatch, loadEditView } = useMatchEdit()

const { data: options, isSuccess: optionsLoaded, isFetching: optionsFetching } = useManualLeagueOptions(deckId, adding)
const leagueOptions = computed(() => leagueOptionsFor(options.value ?? [], deckId.value))

const resultOptions = [
  { value: 'W', label: 'Win' },
  { value: 'L', label: 'Loss' },
]
const playOptions = [
  { value: 'play', label: 'Play' },
  { value: 'draw', label: 'Draw' },
]

watch(startedAt, (value) => {
  endedAt.value = endedAtFor(value, endedAt.value, endedAtTouched.value)
})

// A new deck drops a league it cannot join, once that deck's leagues are in.
watch([leagueOptions, optionsLoaded, optionsFetching], () => {
  if (!optionsLoaded.value || optionsFetching.value || league.value === null) return
  league.value = keepLeague(league.value, leagueOptions.value)
})

function openSheet(preset?: { league?: string; deckId?: number | null }) {
  editingId.value = null
  errors.value = {}
  deckId.value = props.deckId ?? preset?.deckId ?? null
  opponentName.value = ''
  league.value = preset?.league ?? null
  startedAt.value = toLocalInput(new Date())
  endedAt.value = plusMinutes(startedAt.value, MATCH_MINUTES)
  endedAtTouched.value = false
  games.value = emptyGames()
  setAsideGames.value = []
  archetypeId.value = null
  choosingArchetype.value = false
  ranked.value = 'ranked'
  bestOf.value = 'bo3'
  rankGap.value = NO_RANK_GAP
  open.value = true
}

/** "Edit match": open on match row `matchId`, filled from its stored values. */
async function openEdit(matchId: number) {
  const view = await loadEditView(matchId)
  if (view === null) return
  const form = editFormFromView(view)
  openedTimes.value = { startedAt: form.startedAt, endedAt: form.endedAt }
  editingId.value = matchId
  editPlatform.value = view.platform
  errors.value = {}
  opponentName.value = form.opponentName
  // Ended stays as stored: a new start does not move it.
  endedAtTouched.value = true
  startedAt.value = form.startedAt
  endedAt.value = form.endedAt
  games.value = form.games
  choosingArchetype.value = false
  open.value = true
}

function onDeckChange(value: unknown) {
  const parsed = Number(value)
  deckId.value = value === '' || value === null || value === undefined || !Number.isFinite(parsed) ? null : parsed
}

function onLeagueChange(value: unknown) {
  league.value = value === 'none' || typeof value !== 'string' ? null : value
}

function onRankedChange(value: string) {
  ranked.value = value === 'unranked' ? 'unranked' : 'ranked'
}

function onRankGapChange(value: unknown) {
  rankGap.value = typeof value === 'string' ? value : NO_RANK_GAP
}

function onEndedAtInput(value: string | number) {
  endedAtTouched.value = true
  endedAt.value = String(value)
}

/** The rows Bo1 hid, so choosing Bo3 again brings games 2 and 3 back. */
const setAsideGames = ref<GameRow[]>([])
function onBestOfChange(kind: BestOfView) {
  // The switch emits on a click of the active kind too: that changes nothing.
  if (kind === bestOf.value) return
  if (kind === 'bo1') setAsideGames.value = games.value
  bestOf.value = kind
  games.value = gamesForKind(games.value, kind, setAsideGames.value)
}

function setResult(index: number, value: string) {
  games.value = nextResults(games.value, index, value)
}

function setOnPlay(index: number, value: string) {
  games.value[index].onPlay = value as GameRow['onPlay']
}

const shownFields = computed(() => [
  'opponent_name',
  'archetype_id',
  'league_id',
  'started_at',
  'ended_at',
  'games',
  ...(props.deckId === undefined ? ['deck_id'] : []),
  ...(showRankGap.value ? ['rank_gap'] : []),
])
const otherErrors = computed(() => errorsBesides(errors.value, shownFields.value))
const gamesTurnsError = computed(() => turnsError(errors.value))
const canSubmit = computed(() =>
  editing.value
    ? canSubmitEdit({
        opponentName: opponentName.value,
        games: games.value,
        saving: saving.value,
        opponentRequired: !arena.value,
      })
    : canSubmitMatch({
        deckId: deckId.value,
        opponentName: opponentName.value,
        games: games.value,
        saving: saving.value,
        opponentRequired: !arena.value,
      }),
)

async function save(matchId: number) {
  errors.value = {}
  const outcome = await correctMatch(
    matchId,
    correctionPayload(
      {
        opponentName: opponentName.value,
        startedAt: startedAt.value,
        endedAt: endedAt.value,
        games: games.value,
      },
      openedTimes.value,
    ),
  )
  if (outcome.ok) {
    open.value = false
    return
  }
  errors.value = outcome.errors
}

/** Arena's fields: ranked or not, and a ranked match's gap (MTGO sends neither). */
function arenaPayload(): { ranked?: boolean; rankGap?: RankGapKey | null; bestOf?: BestOfView } {
  if (!arena.value) return {}
  const isRanked = ranked.value === 'ranked'
  return {
    ranked: isRanked,
    rankGap: isRanked ? rankGapFor(rankGap.value) : null,
    ...(bestOf.value === 'bo1' ? { bestOf: 'bo1' as const } : {}),
  }
}

async function submit() {
  if (editingId.value !== null) {
    if (canSubmit.value) await save(editingId.value)
    return
  }
  const deck = deckId.value
  if (!canSubmit.value || deck === null) return
  errors.value = {}
  const outcome = await createManual({
    deckId: deck,
    opponentName: opponentName.value,
    league: league.value,
    startedAt: startedAt.value,
    endedAt: endedAt.value,
    games: gamesPayload(games.value),
    archetypeId: archetypeId.value,
    ...arenaPayload(),
  })
  if (outcome.ok) {
    open.value = false
    return
  }
  errors.value = outcome.errors
}

defineExpose({ open: openSheet, openEdit })
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent side="right" class="flex w-[480px] flex-col gap-0 overflow-y-auto p-0 sm:max-w-[480px]">
      <SheetHeader class="border-b border-border p-4">
        <SheetTitle>{{ editing ? 'Edit match' : 'Add manual match' }}</SheetTitle>
        <SheetDescription v-if="editing">
          Fix what the tracker got wrong. Your changes are kept when the match is read again.
        </SheetDescription>
        <SheetDescription v-else>
          Record a match the tracker did not see. It counts everywhere except the shared stats.
        </SheetDescription>
      </SheetHeader>

      <form class="flex flex-1 flex-col gap-5 p-4" @submit.prevent="submit">
        <div v-if="!editing && props.deckId === undefined" class="flex flex-col gap-1.5">
          <Label for="manual-deck">Deck</Label>
          <Select :model-value="deckId !== null ? String(deckId) : ''" @update:model-value="onDeckChange">
            <SelectTrigger id="manual-deck" class="w-full">
              <SelectValue placeholder="Select a deck" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="d in decks" :key="d.id" :value="String(d.id)">
                {{ d.name }} <span class="text-xs text-muted-foreground">({{ d.format }})</span>
              </SelectItem>
            </SelectContent>
          </Select>
          <p v-if="errors.deck_id" class="text-xs text-destructive">{{ errors.deck_id }}</p>
        </div>

        <div class="flex flex-col gap-1.5">
          <Label for="manual-opponent">{{ arena ? 'Opponent (optional)' : 'Opponent' }}</Label>
          <Input
            id="manual-opponent"
            v-model="opponentName"
            :placeholder="arena ? 'Arena name' : 'MTGO username'"
            autocomplete="off"
          />
          <p v-if="errors.opponent_name" class="text-xs text-destructive">{{ errors.opponent_name }}</p>
        </div>

        <div v-if="!editing" class="flex flex-col gap-1.5">
          <Label>Opponent archetype</Label>
          <div class="flex items-center gap-2">
            <span class="flex-1 truncate text-sm" :class="{ 'text-muted-foreground italic': archetypeName === null }">
              {{ archetypeName ?? 'Unknown archetype' }}
            </span>
            <Button type="button" size="sm" variant="outline" @click="choosingArchetype = !choosingArchetype">
              {{ choosingArchetype ? 'Cancel' : 'Choose' }}
            </Button>
            <Button v-if="archetypeId !== null" type="button" size="sm" variant="ghost" @click="archetypeId = null">
              Clear
            </Button>
          </div>
          <ArchetypePicker
            v-if="choosingArchetype"
            :archetypes="archetypeOptions"
            :format="deckFormat"
            :current-id="archetypeId"
            show-fallbacks
            @select="chooseArchetype"
          />
          <p v-if="errors.archetype_id" class="text-xs text-destructive">{{ errors.archetype_id }}</p>
        </div>

        <div v-if="!editing && arena" class="flex flex-col gap-1.5">
          <Label>Queue</Label>
          <SegmentedControl :model-value="ranked" :options="rankedOptions" @update:model-value="onRankedChange" />
        </div>

        <div v-if="!editing && arena" class="flex flex-col gap-1.5">
          <Label>Match kind</Label>
          <BestOfSwitch class="w-fit" :model-value="bestOf" @update:model-value="onBestOfChange" />
        </div>

        <div v-if="showRankGap" class="flex flex-col gap-1.5">
          <Label for="manual-rank-gap">Rank gap</Label>
          <Select data-testid="manual-rank-gap" :model-value="rankGap" @update:model-value="onRankGapChange">
            <SelectTrigger id="manual-rank-gap" class="w-full">
              <SelectValue placeholder="None" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem :value="NO_RANK_GAP">None</SelectItem>
              <SelectItem v-for="gap in gapOptions" :key="rankGapValue(gap.key)" :value="rankGapValue(gap.key)">
                {{ gap.label }}
              </SelectItem>
            </SelectContent>
          </Select>
          <p class="text-xs text-muted-foreground">
            The stretch between two rank readings this match was played in, when the tracker saw none.
          </p>
          <p v-if="errors.rank_gap" class="text-xs text-destructive">{{ errors.rank_gap }}</p>
        </div>

        <div v-if="!editing && !arena" class="flex flex-col gap-1.5">
          <Label for="manual-league">League</Label>
          <Select
            :model-value="league ?? 'none'"
            :disabled="deckId === null"
            @update:model-value="onLeagueChange"
          >
            <SelectTrigger id="manual-league" class="w-full">
              <SelectValue placeholder="No league" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="none">No league</SelectItem>
              <SelectItem v-for="l in leagueOptions" :key="l.key" :value="l.key">
                {{ l.name }}
                <span class="text-xs text-muted-foreground">({{ l.matchCount }}/{{ l.roundCount }})</span>
              </SelectItem>
            </SelectContent>
          </Select>
          <p v-if="errors.league_id" class="text-xs text-destructive">{{ errors.league_id }}</p>
        </div>

        <div class="grid grid-cols-2 gap-3">
          <div class="flex flex-col gap-1.5">
            <Label for="manual-started">Started</Label>
            <Input id="manual-started" v-model="startedAt" type="datetime-local" />
            <p v-if="errors.started_at" class="text-xs text-destructive">{{ errors.started_at }}</p>
          </div>
          <div class="flex flex-col gap-1.5">
            <Label for="manual-ended">Ended</Label>
            <Input id="manual-ended" :model-value="endedAt" type="datetime-local" @update:model-value="onEndedAtInput" />
            <p v-if="errors.ended_at" class="text-xs text-destructive">{{ errors.ended_at }}</p>
          </div>
        </div>

        <div class="flex flex-col gap-2">
          <Label>Games</Label>
          <div
            v-for="(game, index) in games"
            :key="index"
            class="flex items-center justify-between gap-3 rounded-md border border-border p-2 transition-opacity"
            :class="isGameEnabled(games, index) ? '' : 'opacity-50'"
          >
            <span class="w-14 text-xs font-medium text-muted-foreground">Game {{ index + 1 }}</span>
            <SegmentedControl
              :model-value="game.result"
              :options="resultOptions"
              :disabled="!isGameEnabled(games, index)"
              @update:model-value="setResult(index, $event)"
            />
            <SegmentedControl
              :model-value="game.onPlay"
              :options="playOptions"
              :disabled="!isGameEnabled(games, index)"
              @update:model-value="setOnPlay(index, $event)"
            />
            <Input
              v-model="game.turns"
              type="number"
              inputmode="numeric"
              min="1"
              max="99"
              placeholder="Turns"
              :aria-label="`Game ${index + 1} turns`"
              :disabled="!isGameEnabled(games, index)"
              class="h-8 w-20 text-xs"
            />
          </div>
          <p class="text-xs text-muted-foreground">
            Games unlock in order and stop at two wins. A 1-1 after two games is saved as a draw. Click a result
            again to clear it. Turns are optional.
          </p>
          <p v-if="errors.games" class="text-xs text-destructive">{{ errors.games }}</p>
          <p v-if="gamesTurnsError" class="text-xs text-destructive">{{ gamesTurnsError }}</p>
        </div>

        <p v-for="message in otherErrors" :key="message" class="text-xs text-destructive">{{ message }}</p>

        <SheetFooter class="mt-auto flex-row justify-end gap-2 border-t border-border pt-4">
          <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
          <Button type="submit" :disabled="!canSubmit">{{ editing ? 'Save' : 'Add match' }}</Button>
        </SheetFooter>
      </form>
    </SheetContent>
  </Sheet>
</template>
