<script setup lang="ts">
import { ArrowDown, ArrowUp, ArrowUpDown, Layers, LoaderCircle, NotepadText, ScanSearch, Tags, TrendingDown, TrendingUp, TriangleAlert, X } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRouter, type RouteLocationRaw } from 'vue-router'
import type { MatchSummary } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import ManualMatchBadge from '@/components/matches/ManualMatchBadge.vue'
import ManualMatchSheet from '@/components/matches/ManualMatchSheet.vue'
import MatchNotesDialog from '@/components/matches/MatchNotesDialog.vue'
import MatchRowContextMenu from '@/components/matches/MatchRowContextMenu.vue'
import SetArchetypeDialog from '@/components/matches/SetArchetypeDialog.vue'
import ArchetypeName from '@/components/archetypes/ArchetypeName.vue'
import MoveToLeagueDialog from '@/components/matches/MoveToLeagueDialog.vue'
import SetDeckDialog from '@/components/matches/SetDeckDialog.vue'
import ResultBadge from '@/components/ResultBadge.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { useMatchEdit } from '@/composables/useMatchEdit'
import { useSetOpponentArchetype } from '@/composables/useOpponentArchetype'
import { bulkFormat } from '@/lib/archetypePicker'
import { outcomeLabel, stateBadge, type MatchDeck } from '@/lib/format'
import { colourLetters } from '@/lib/limited'
import { previews } from '@/lib/liveEvents'
import { arenaKindLabel, arenaTypeLabel, arenaRankCell, endstepRatingCell, gameCells, matchDateText, matchDurationText, rowMenuEnabled, type SortColumn, type SortDir } from '@/lib/matchList'

/**
 * The matches table (0.x `MatchesTable`): result, deck (optional), opponent,
 * the opponent archetype and its colours, games 1 to 3, duration, date and
 * the notes icon. A row opens its match; a right click (not on a match still in progress) offers notes, the
 * archetype items (set by hand, clear, "Detect" when `detectable`), "Set
 * deck..." (issue #332), "Move to league..." for a finished match and
 * "Delete match" (league self-management spec 5.4). With `sortable`, the
 * Result, Archetype, Game, Duration and Date headers emit `sort`. With
 * `selectable`, rows have checkboxes and the selection bar offers a bulk
 * "Set archetype" and "Set deck", and "Detect archetypes" when
 * `detectable` (emits `detect` with the selected tokens). A Limited
 * page shows the opponent's colours instead (0.x `showArchetype=false`) and
 * opens its rows inside the event (0.x `matchUrl`).
 */
const props = withDefaults(
  defineProps<{
    rows: MatchSummary[]
    /** Each row's deck by match token; given, the table shows a Deck column (0.x `showDeck`). */
    decks?: Record<string, MatchDeck>
    sortable?: boolean
    sortBy?: SortColumn | null
    sortDir?: SortDir
    /** A draft seat has no archetype: its colours are the only label (0.x `showArchetype=false`). */
    showColours?: boolean
    /** Where a row click goes; defaults to the match page. */
    matchRoute?: (token: string) => string
    /** Rows have checkboxes and the bulk "Set archetype" bar. */
    selectable?: boolean
    /** The page detects archetypes: rows and the selection bar offer "Detect" (emits `detect`). */
    detectable?: boolean
    /** Tokens of the matches being detected now: their archetype cell spins (0.x `detectingMatchId`). */
    detecting?: string[]
    /** One game per match (Bo1): the Game 1 to 3 columns go and the Result column says it all. */
    singleResult?: boolean
    /** Leave the Bo1/Bo3 label out: the page already shows the kind (a deck page). */
    hideKind?: boolean
  }>(),
  {
    decks: undefined,
    sortable: false,
    sortBy: null,
    sortDir: 'desc',
    showColours: false,
    matchRoute: undefined,
    selectable: false,
    detectable: false,
    detecting: () => [],
    singleResult: false,
    hideKind: false,
  },
)

const emit = defineEmits<{ sort: [column: SortColumn]; detect: [tokens: string[]] }>()

const archetypeDialog = ref<InstanceType<typeof SetArchetypeDialog> | null>(null)
const deckDialog = ref<InstanceType<typeof SetDeckDialog> | null>(null)
const moveDialog = ref<InstanceType<typeof MoveToLeagueDialog> | null>(null)
const editSheet = ref<InstanceType<typeof ManualMatchSheet> | null>(null)
const { setArchetype } = useSetOpponentArchetype()
const showArchetype = computed(() => !props.showColours)
// Arena and Endstep rows get a Rank column and a Bo column (not where the page shows the kind); MTGO tables keep their columns.
const showRank = computed(() => props.rows.some((row) => row.platform !== 'mtgo'))
const showBo = computed(() => showRank.value && !props.hideKind)
const canSelect = computed(() => props.selectable && showArchetype.value)
const selected = ref<string[]>([])

// A new page or filter starts with nothing selected.
watch(
  () => props.rows.map((row) => row.token).join(','),
  () => {
    selected.value = []
  },
)

const allSelected = computed(() => props.rows.length > 0 && selected.value.length === props.rows.length)
const someSelected = computed(() => selected.value.length > 0 && !allSelected.value)

function toggleAll(checked: boolean | 'indeterminate') {
  selected.value = checked === true ? props.rows.map((row) => row.token) : []
}

function toggleRow(token: string, checked: boolean | 'indeterminate') {
  selected.value =
    checked === true ? [...new Set([...selected.value, token])] : selected.value.filter((shown) => shown !== token)
}

function openBulkSetArchetype() {
  const formats = props.rows.filter((row) => selected.value.includes(row.token)).map((row) => row.format)
  archetypeDialog.value?.openForMatches(selected.value, bulkFormat(formats))
}

function setRowArchetype(row: MatchSummary) {
  archetypeDialog.value?.openForMatch(row.token, row.format, row.opponentArchetype?.id ?? null)
}

function openBulkSetDeck() {
  deckDialog.value?.openForMatches(selected.value)
}

// In page order (not ticking order), which the worker detects in.
function bulkDetect() {
  emit(
    'detect',
    props.rows.map((row) => row.token).filter((token) => selected.value.includes(token)),
  )
  selected.value = []
}

async function clearRowArchetype(row: MatchSummary) {
  await setArchetype([row.token], null)
}

const router = useRouter()
const notesDialog = ref<InstanceType<typeof MatchNotesDialog> | null>(null)
const removing = ref<string | null>(null)
const { saving, hide } = useMatchEdit()

const GAME_COLUMNS: { column: SortColumn; label: string }[] = [
  { column: 'game_1', label: 'Game 1' },
  { column: 'game_2', label: 'Game 2' },
  { column: 'game_3', label: 'Game 3' },
]

const gameColumns = computed(() => (props.singleResult ? [] : GAME_COLUMNS))

function gameCellsShown(row: MatchSummary) {
  return props.singleResult ? [] : gameCells(shown(row).gameResults)
}

/** The deck dashboard of a row's deck; null when the match has no deck. */
function deckLink(token: string): { to: RouteLocationRaw; name: string } | null {
  const deck = props.decks?.[token]
  if (!deck || deck.id === null) return null
  return { to: { name: 'deck-dashboard', params: { id: deck.id } }, name: deck.name ?? 'Unknown' }
}

/** A row being updated shows what it will look like once committed (a preview carries no rank: keep the row's). */
function shown(row: MatchSummary): MatchSummary {
  const preview = previews.value.get(row.token)
  return preview ? { ...preview, rank: row.rank, rating: row.rating } : row
}

function sortIcon(column: SortColumn) {
  if (props.sortBy !== column) return ArrowUpDown
  return props.sortDir === 'desc' ? ArrowDown : ArrowUp
}

function clickSort(column: SortColumn) {
  if (props.sortable) emit('sort', column)
}

function openMatch(token: string) {
  void router.push(props.matchRoute ? props.matchRoute(token) : `/matches/${encodeURIComponent(token)}`)
}

async function confirmRemove() {
  const token = removing.value
  if (token === null) return
  if (await hide(token)) removing.value = null
}
</script>

<template>
  <MatchNotesDialog ref="notesDialog" />
  <SetArchetypeDialog ref="archetypeDialog" @archetype-set="selected = []" />
  <SetDeckDialog ref="deckDialog" @deck-set="selected = []" />
  <MoveToLeagueDialog ref="moveDialog" />
  <ManualMatchSheet ref="editSheet" />

  <div v-if="canSelect && selected.length > 0" class="flex items-center gap-3 border-b bg-muted/50 px-4 py-2">
    <span class="text-sm font-medium">{{ selected.length }} selected</span>
    <Button variant="outline" size="sm" class="gap-1.5" @click="openBulkSetArchetype">
      <Tags class="size-3.5" />
      Set archetype
    </Button>
    <Button variant="outline" size="sm" class="gap-1.5" @click="openBulkSetDeck">
      <Layers class="size-3.5" />
      Set deck
    </Button>
    <Button v-if="detectable" variant="outline" size="sm" class="gap-1.5" data-testid="bulk-detect" @click="bulkDetect">
      <ScanSearch class="size-3.5" />
      Detect archetypes
    </Button>
    <Button variant="ghost" size="sm" class="ml-auto gap-1.5 text-muted-foreground" @click="selected = []">
      <X class="size-3.5" />
      Clear selection
    </Button>
  </div>

  <Dialog :open="removing !== null" @update:open="(open) => !open && (removing = null)">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Delete match?</DialogTitle>
        <DialogDescription>
          The match leaves every list, deck, league and number in the tracker. You can restore it from Settings,
          Storage. MTGO's own records are not changed.
        </DialogDescription>
      </DialogHeader>
      <DialogFooter>
        <Button variant="outline" size="sm" @click="removing = null">Cancel</Button>
        <Button variant="destructive" size="sm" :disabled="saving" @click="confirmRemove">Delete match</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>

  <TooltipProvider>
    <Table>
      <TableHeader class="sticky top-0 z-10 backdrop-blur-sm">
        <TableRow>
          <TableHead v-if="canSelect" class="w-8" @click.stop>
            <Checkbox
              :model-value="allSelected ? true : someSelected ? 'indeterminate' : false"
              aria-label="Select all"
              @update:model-value="toggleAll"
            />
          </TableHead>
          <TableHead :class="{ 'cursor-pointer select-none': sortable }" @click="clickSort('outcome')">
            <span class="flex items-center gap-1">
              Result
              <component
                :is="sortIcon('outcome')"
                v-if="sortable"
                class="size-3"
                :class="{ 'opacity-30': props.sortBy !== 'outcome' }"
              />
            </span>
          </TableHead>
          <TableHead v-if="decks">Deck</TableHead>
          <TableHead>Opponent</TableHead>
          <TableHead v-if="showBo">Bo</TableHead>
          <TableHead v-if="showRank">Rank</TableHead>
          <template v-if="showArchetype">
            <TableHead :class="{ 'cursor-pointer select-none': sortable }" @click="clickSort('archetype')">
              <span class="flex items-center gap-1">
                Archetype
                <component
                  :is="sortIcon('archetype')"
                  v-if="sortable"
                  class="size-3"
                  :class="{ 'opacity-30': props.sortBy !== 'archetype' }"
                />
              </span>
            </TableHead>
            <TableHead>Colours</TableHead>
          </template>
          <TableHead v-else>Colours</TableHead>
          <TableHead
            v-for="game in gameColumns"
            :key="game.column"
            :class="{ 'cursor-pointer select-none': sortable }"
            @click="clickSort(game.column)"
          >
            <span class="flex items-center gap-1">
              {{ game.label }}
              <component
                :is="sortIcon(game.column)"
                v-if="sortable"
                class="size-3"
                :class="{ 'opacity-30': props.sortBy !== game.column }"
              />
            </span>
          </TableHead>
          <TableHead :class="{ 'cursor-pointer select-none': sortable }" @click="clickSort('duration')">
            <span class="flex items-center gap-1">
              Duration
              <component
                :is="sortIcon('duration')"
                v-if="sortable"
                class="size-3"
                :class="{ 'opacity-30': props.sortBy !== 'duration' }"
              />
            </span>
          </TableHead>
          <TableHead :class="{ 'cursor-pointer select-none': sortable }" @click="clickSort('started_at')">
            <span class="flex items-center gap-1">
              Date
              <component
                :is="sortIcon('started_at')"
                v-if="sortable"
                class="size-3"
                :class="{ 'opacity-30': props.sortBy !== 'started_at' }"
              />
            </span>
          </TableHead>
          <TableHead></TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <MatchRowContextMenu
          v-for="row in rows"
          :key="row.token"
          :notes="row.notes"
          :editable="row.id !== null"
          :archetypes="showArchetype"
          :has-archetype="row.opponentArchetype !== null"
          :detectable="detectable"
          :deck="showArchetype && !row.limited"
          :disabled="!rowMenuEnabled(row)"
          :movable="row.state === 'complete' || row.state === 'abandoned'"
          @edit="row.id !== null && editSheet?.openEdit(row.id)"
          @open-notes="notesDialog?.openForMatch(row.token, row.notes)"
          @remove="removing = row.token"
          @set-archetype="setRowArchetype(row)"
          @clear-archetype="clearRowArchetype(row)"
          @detect="emit('detect', [row.token])"
          @set-deck="deckDialog?.openForMatches([row.token])"
          @move-to-league="moveDialog?.openForMatch(row.token)"
        >
          <TableRow
            class="cursor-pointer"
            data-testid="match-row"
            :data-state="selected.includes(row.token) ? 'selected' : undefined"
            @click="openMatch(row.token)"
          >
            <TableCell v-if="canSelect" @click.stop>
              <Checkbox
                :model-value="selected.includes(row.token)"
                aria-label="Select match"
                @update:model-value="(checked) => toggleRow(row.token, checked)"
              />
            </TableCell>
            <TableCell><ResultBadge v-bind="outcomeLabel(shown(row))" /></TableCell>
            <TableCell v-if="decks">
              <RouterLink
                v-if="deckLink(row.token)"
                :to="deckLink(row.token)!.to"
                class="text-primary hover:underline"
                @click.stop
              >
                {{ deckLink(row.token)!.name }}
              </RouterLink>
              <span v-else class="text-muted-foreground">Unknown</span>
            </TableCell>
            <TableCell class="font-medium">
              <span class="inline-flex items-center gap-1.5">
                <template v-if="shown(row).opponent">
                  {{ shown(row).opponent }}
                  <ManualMatchBadge v-if="shown(row).manual" />
                </template>
                <span v-else class="text-xs text-muted-foreground">-</span>
              </span>
            </TableCell>
            <TableCell v-if="showBo" data-cell="bo" class="text-muted-foreground">{{ arenaKindLabel(shown(row)) ?? '' }}</TableCell>
            <TableCell v-if="showRank" data-cell="rank">
              <span v-if="shown(row).platform === 'endstep'" data-testid="rating" class="inline-flex items-center gap-2 tabular-nums">
                <template v-if="shown(row).rating">
                  <span>{{ endstepRatingCell(shown(row).rating!).after }}</span>
                  <span class="inline-flex items-center gap-0.5 text-xs" :class="endstepRatingCell(shown(row).rating!).tone">
                    <TrendingUp v-if="endstepRatingCell(shown(row).rating!).trend === 'up'" class="size-3.5" aria-hidden="true" />
                    <TrendingDown v-else-if="endstepRatingCell(shown(row).rating!).trend === 'down'" class="size-3.5" aria-hidden="true" />
                    {{ endstepRatingCell(shown(row).rating!).change }}
                  </span>
                </template>
                <span v-else-if="arenaTypeLabel(shown(row))" class="text-muted-foreground">{{ arenaTypeLabel(shown(row)) }}</span>
              </span>
              <span
                v-else-if="arenaRankCell(shown(row))"
                class="inline-flex items-center gap-1.5"
                :class="{ 'text-muted-foreground': arenaRankCell(shown(row))!.muted }"
              >
                <img
                  v-if="arenaRankCell(shown(row))!.iconUrl"
                  :src="arenaRankCell(shown(row))!.iconUrl!"
                  :alt="arenaRankCell(shown(row))!.label"
                  :title="arenaRankCell(shown(row))!.label"
                  class="size-5 shrink-0"
                  data-testid="rank-icon"
                />
                {{ arenaRankCell(shown(row))!.text }}
              </span>
            </TableCell>
            <template v-if="showArchetype">
              <TableCell data-cell="archetype">
                <LoaderCircle
                  v-if="detecting.includes(row.token)"
                  class="size-3.5 animate-spin text-muted-foreground"
                  aria-label="Detecting"
                />
                <ArchetypeName v-else-if="row.opponentArchetype" :archetype="row.opponentArchetype" />
                <span v-else class="text-muted-foreground">Unknown</span>
              </TableCell>
              <TableCell>
                <ManaColors v-if="row.opponentArchetype?.colors.length" :colors="row.opponentArchetype.colors" />
              </TableCell>
            </template>
            <TableCell v-else>
              <ManaColors
                v-if="colourLetters(row.opponentColors).length"
                :colors="colourLetters(row.opponentColors)"
              />
              <span v-else class="text-xs text-muted-foreground">-</span>
            </TableCell>
            <TableCell v-for="(cell, index) in gameCellsShown(row)" :key="index" class="text-sm">
              <template v-if="cell">
                <span :class="cell.tone">{{ cell.text }}</span>
                <span v-if="cell.side" class="text-xs text-muted-foreground"> {{ cell.side }}</span>
              </template>
              <span v-else class="text-muted-foreground">-</span>
            </TableCell>
            <TableCell>
              <span v-if="matchDurationText(shown(row).startedAtMs, shown(row).endedAtMs)">
                {{ matchDurationText(shown(row).startedAtMs, shown(row).endedAtMs) }}
              </span>
              <span v-else class="text-muted-foreground">-</span>
            </TableCell>
            <TableCell class="whitespace-nowrap">{{ matchDateText(shown(row).startedAtMs) }}</TableCell>
            <TableCell>
              <div class="flex items-center justify-end gap-2">
                <LoaderCircle
                  v-if="previews.has(row.token)"
                  class="size-3.5 animate-spin text-muted-foreground"
                  aria-label="Updating"
                />
                <Badge v-if="stateBadge(shown(row))" variant="success">{{ stateBadge(shown(row)) }}</Badge>
                <Tooltip v-if="row.needsAttention">
                  <TooltipTrigger as-child>
                    <TriangleAlert class="size-4 text-warning" aria-label="Needs attention" />
                  </TooltipTrigger>
                  <TooltipContent side="left" class="max-w-xs">
                    <p class="text-xs">{{ row.attentionReason ?? 'This match could not be read in full.' }}</p>
                  </TooltipContent>
                </Tooltip>
                <Tooltip v-if="row.notes">
                  <TooltipTrigger as-child>
                    <NotepadText :size="14" class="text-muted-foreground" aria-label="Notes" />
                  </TooltipTrigger>
                  <TooltipContent side="left" class="max-w-xs">
                    <p class="text-xs whitespace-pre-wrap">{{ row.notes }}</p>
                  </TooltipContent>
                </Tooltip>
              </div>
            </TableCell>
          </TableRow>
        </MatchRowContextMenu>
      </TableBody>
    </Table>
  </TooltipProvider>
</template>
