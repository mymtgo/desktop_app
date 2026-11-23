<script setup lang="ts">
import { AlertTriangle, Check, ChevronLeft, ChevronRight, FileUp, Trash2, X } from 'lucide-vue-next'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  commands,
  events,
  type ConfidenceStats,
  type HistoryImportResult,
  type HistoryScanMatch,
  type HistoryScanStatus,
  type HistorySort,
} from '@/bindings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import ScanCardsDialog from '@/components/import/ScanCardsDialog.vue'
import LegacyImportCard from '@/components/LegacyImportCard.vue'
import ScanResultsTable from '@/components/import/ScanResultsTable.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Spinner } from '@/components/ui/spinner'
import { errorMessage, unwrap } from '@/lib/commands'
import {
  DEFAULT_MIN_CONFIDENCE,
  SORT_OPTIONS,
  deckVersionLabel,
  emptyLine,
  importAllFilters,
  importResultLine,
  isStaleStatus,
  minConfidenceLabel,
  pageStateFor,
  progressLine,
  progressPercent,
  scanQuery,
  stageLabel,
  type ImportPageState,
  type ResultFilters,
} from '@/lib/historyImport'
import { ALL_FORMATS } from '@/lib/decks'
import { keys, useHistoryDeckVersions, useHistoryImportedCount } from '@/lib/queries'
import { queryClient } from '@/queryClient'

// A port of 0.x `pages/import/Index.vue`. The scan lives in the app's memory,
// so the page starts from its status, follows `HistoryScanChanged` pushes, and
// polls every 2 s while a scan runs in case a push is missed (0.x polled).

const { data: deckVersions } = useHistoryDeckVersions()
const { data: importedCountData } = useHistoryImportedCount()
const importedCount = computed(() => importedCountData.value ?? 0)

// State
/** False until the app's scan status is known. */
const ready = ref(false)
const state = ref<ImportPageState>('setup')
/** The last scan status applied. */
const shown = ref<HistoryScanStatus | null>(null)
const selectedDeckId = ref<number | null>(null)
const scanDeckName = ref('')
const scanProgress = ref(0)
const scanTotal = ref(0)
const scanStage = ref('discovering')
const scanError = ref<string | null>(null)

const rows = ref<HistoryScanMatch[]>([])
const currentPage = ref(1)
const lastPage = ref(1)
const totalMatches = ref(0)
const selectedIds = ref<Set<number>>(new Set())

const errorText = ref<string | null>(null)
const importResult = ref<HistoryImportResult | null>(null)
const importing = ref(false)

const formatFilter = ref('')
function onFormatFilter(value: unknown) {
  const next = String(value ?? '')
  formatFilter.value = next === ALL_FORMATS ? '' : next
}
const availableFormats = ref<string[]>([])
const confidenceStats = ref<ConfidenceStats | null>(null)
// Default 0.1, to leave out matches with no game log.
const minConfidence = ref(DEFAULT_MIN_CONFIDENCE)
const sortBy = ref<HistorySort>('confidence')

const filters = computed<ResultFilters>(() => ({
  format: formatFilter.value,
  minConfidence: minConfidence.value,
  sort: sortBy.value,
}))

const activeDeckVersions = computed(() => (deckVersions.value ?? []).filter((d) => !d.archived))
const deletedDeckVersions = computed(() => (deckVersions.value ?? []).filter((d) => d.archived))
const selectedCount = computed(() => selectedIds.value.size)
const selectedDeckName = computed(
  () => (deckVersions.value ?? []).find((d) => d.id === selectedDeckId.value)?.deckName ?? '',
)

// Scan status

function applyStatus(status: HistoryScanStatus | null) {
  if (isStaleStatus(shown.value, status)) return
  shown.value = status
  if (status === null) {
    // Forgotten (Start over, here or in another window).
    stopPolling()
    if (state.value !== 'setup') resetResults()
    state.value = 'setup'
    return
  }
  scanDeckName.value = status.deckName
  scanProgress.value = status.progress
  scanTotal.value = status.total
  scanStage.value = status.stage
  switch (status.status) {
    case 'processing':
      state.value = 'processing'
      startPolling()
      break
    case 'complete':
      stopPolling()
      availableFormats.value = status.formats
      confidenceStats.value = status.confidenceStats
      if (state.value !== 'results') {
        totalMatches.value = status.matchCount
        state.value = 'results'
        void loadMatches(1)
      }
      break
    case 'failed':
      stopPolling()
      scanError.value = status.error ?? 'Scan failed'
      state.value = 'setup'
      break
    case 'cancelled':
      stopPolling()
      state.value = 'setup'
      break
  }
}

let pollTimer: ReturnType<typeof setInterval> | null = null

function startPolling() {
  if (pollTimer !== null) return
  pollTimer = setInterval(() => void pollStatus(), 2000)
}

function stopPolling() {
  if (pollTimer !== null) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

async function pollStatus() {
  try {
    applyStatus(unwrap(await commands.historyScanStatus()))
  } catch {
    // Silently retry, as 0.x.
  }
}

/** Pushes received, so the first status fetch never overwrites a newer push. */
let pushes = 0
const listening = useTauriEvents(() => [
  events.historyScanChanged.listen((e) => {
    pushes += 1
    applyStatus(e.payload.status)
  }),
  // Imported rows leave the results: refetch the page shown.
  events.historyImportsChanged.listen(() => void loadMatches(currentPage.value)),
])

onMounted(async () => {
  if (!(await listening.ready)) return
  const before = pushes
  try {
    const status = unwrap(await commands.historyScanStatus())
    if (!listening.isMounted()) return
    selectedDeckId.value = status?.deckVersionId ?? null
    if (pushes === before) applyStatus(status)
    // The first page state comes from the app's scan, as 0.x's did from its props.
    state.value = pageStateFor(shown.value)
  } catch (error) {
    errorText.value = errorMessage(error)
  } finally {
    ready.value = true
  }
})

onUnmounted(() => {
  stopPolling()
  if (filterTimeout !== null) clearTimeout(filterTimeout)
})

async function startScan() {
  if (selectedDeckId.value === null) return

  errorText.value = null
  importResult.value = null
  scanError.value = null
  scanDeckName.value = selectedDeckName.value

  try {
    const status = unwrap(await commands.startHistoryScan(selectedDeckId.value))
    scanProgress.value = 0
    scanTotal.value = 0
    applyStatus(status)
  } catch (error) {
    errorText.value = `Scan failed: ${errorMessage(error)}`
  }
}

function resetResults() {
  rows.value = []
  selectedIds.value = new Set()
  totalMatches.value = 0
  confidenceStats.value = null
  availableFormats.value = []
  formatFilter.value = ''
  // As 0.x: Start over leaves the slider Off.
  minConfidence.value = 0
}

async function cancelScanAndReset() {
  try {
    unwrap(await commands.cancelHistoryScan())
  } catch {
    // Best effort
  }
  stopPolling()
  state.value = 'setup'
  shown.value = null
  resetResults()
}

// Results

/** Only the latest request may fill the table. */
let loadRequest = 0

async function loadMatches(page = 1) {
  if (state.value !== 'results') return
  const request = ++loadRequest
  try {
    const data = unwrap(await commands.historyScanMatches(scanQuery(filters.value, page)))
    if (request !== loadRequest) return
    // Importing the last rows of the last page leaves it past the end.
    if (data.rows.length === 0 && data.page > data.lastPage) {
      void loadMatches(data.lastPage)
      return
    }
    rows.value = data.rows
    currentPage.value = data.page
    lastPage.value = data.lastPage
    totalMatches.value = data.total
  } catch (error) {
    if (request === loadRequest) errorText.value = `Failed to load matches: ${errorMessage(error)}`
  }
}

// Debounced watchers for filters
let filterTimeout: ReturnType<typeof setTimeout> | null = null

watch([formatFilter, minConfidence, sortBy], () => {
  if (filterTimeout !== null) clearTimeout(filterTimeout)
  filterTimeout = setTimeout(() => {
    filterTimeout = null
    currentPage.value = 1
    void loadMatches(1)
  }, 300)
})

// Selection
function toggleSelect(historyId: number) {
  const next = new Set(selectedIds.value)
  if (next.has(historyId)) {
    next.delete(historyId)
  } else {
    next.add(historyId)
  }
  selectedIds.value = next
}

function togglePageSelect() {
  const next = new Set(selectedIds.value)
  const allSelected = rows.value.length > 0 && rows.value.every((m) => next.has(m.historyId))
  for (const m of rows.value) {
    if (allSelected) {
      next.delete(m.historyId)
    } else {
      next.add(m.historyId)
    }
  }
  selectedIds.value = next
}

// Import. Both commands answer once the matches are stored (0.x queued a job
// and polled the list until it emptied).
async function runImport(action: () => Promise<HistoryImportResult>) {
  importing.value = true
  errorText.value = null
  try {
    importResult.value = await action()
    selectedIds.value = new Set()
  } catch (error) {
    errorText.value = `Import failed: ${errorMessage(error)}`
  } finally {
    importing.value = false
    void loadMatches(currentPage.value)
  }
}

// Both imports name the scan whose results are shown: the app refuses them
// when another scan has replaced it.
function importSelected() {
  const scanId = shown.value?.scanId
  if (selectedIds.value.size === 0 || scanId === undefined) return
  const ids = [...selectedIds.value]
  return runImport(async () => unwrap(await commands.importHistory(scanId, ids)))
}

const importAllConfirmOpen = ref(false)

function importAll() {
  importAllConfirmOpen.value = false
  const scanId = shown.value?.scanId
  if (scanId === undefined) return
  const chosen = importAllFilters(filters.value)
  return runImport(async () => unwrap(await commands.importHistoryAll(scanId, chosen)))
}

// Cards modal
const cardsModalOpen = ref(false)
const cardsModalMatch = ref<HistoryScanMatch | null>(null)

function showCards(match: HistoryScanMatch) {
  cardsModalMatch.value = match
  cardsModalOpen.value = true
}

// Delete all imported
const deleteModalOpen = ref(false)
const deleting = ref(false)

async function deleteAllImported() {
  deleting.value = true
  errorText.value = null
  try {
    unwrap(await commands.deleteHistoryImports())
    queryClient.setQueryData(keys.historyImportedCount, 0)
    deleteModalOpen.value = false
  } catch (error) {
    errorText.value = `Delete failed: ${errorMessage(error)}`
  } finally {
    deleting.value = false
  }
}
</script>

<template>
  <div class="flex flex-1 flex-col">
    <div class="mx-auto w-full max-w-6xl space-y-6 p-6">
      <div>
        <h1 class="text-2xl font-bold tracking-tight">Import Match History</h1>
        <p class="text-sm text-muted-foreground">Backfill matches from MTGO's local history files into your database.</p>
      </div>

      <!-- The 0.x import, beside the history import (release spec section 4, ruling 4) -->
      <LegacyImportCard />

      <!-- Warning banner -->
      <Card class="border-yellow-500/50 bg-yellow-500/5 py-0">
        <CardContent class="flex items-start gap-3 p-3">
          <AlertTriangle class="mt-0.5 size-5 shrink-0 text-yellow-500" />
          <p class="text-sm">
            <strong>Imported matches have reduced data fidelity.</strong> Opening hands, sideboard changes, game
            timelines, and turn estimates will not be available. Card game statistics will be approximate: cards are
            counted as "seen" based on game log mentions, not zone tracking.
          </p>
        </CardContent>
      </Card>

      <!-- Imported count + delete -->
      <div v-if="importedCount > 0" class="flex items-center justify-between rounded-md border border-border px-4 py-3">
        <p class="text-sm text-muted-foreground">
          <strong>{{ importedCount }}</strong> previously imported match(es) in the database.
        </p>
        <Button variant="outline" size="sm" class="text-red-500 hover:text-red-600" @click="deleteModalOpen = true">
          <Trash2 class="mr-1.5 size-3.5" />
          Delete all imported
        </Button>
      </div>

      <!-- Error banner -->
      <Card v-if="errorText || scanError" class="border-red-500/50 bg-red-500/5">
        <CardContent class="flex items-center gap-3 p-4">
          <AlertTriangle class="size-5 shrink-0 text-red-500" />
          <p class="text-sm text-red-600 dark:text-red-400">{{ errorText || scanError }}</p>
        </CardContent>
      </Card>

      <!-- Import result banner -->
      <Card v-if="importResult" class="border-green-500/50 bg-green-500/5">
        <CardContent class="flex items-center gap-3 p-4">
          <Check class="size-5 text-green-500" />
          <p class="text-sm">{{ importResultLine(importResult) }}</p>
        </CardContent>
      </Card>

      <!-- STATE 1: Setup -->
      <template v-if="ready && state === 'setup'">
        <Card>
          <CardContent class="space-y-4 p-6">
            <div>
              <label for="import-deck" class="mb-2 block text-sm font-medium">Select a deck</label>
              <Select
                :model-value="selectedDeckId ?? undefined"
                @update:model-value="(value) => (selectedDeckId = typeof value === 'number' ? value : null)"
              >
                <SelectTrigger id="import-deck" class="w-full max-w-md">
                  <SelectValue placeholder="Choose a deck version..." />
                </SelectTrigger>
                <SelectContent>
                  <SelectGroup v-if="activeDeckVersions.length">
                    <SelectLabel>Active Decks</SelectLabel>
                    <SelectItem v-for="dv in activeDeckVersions" :key="dv.id" :value="dv.id">
                      {{ deckVersionLabel(dv) }}
                    </SelectItem>
                  </SelectGroup>
                  <SelectGroup v-if="deletedDeckVersions.length">
                    <SelectLabel>Deleted Decks</SelectLabel>
                    <SelectItem v-for="dv in deletedDeckVersions" :key="dv.id" :value="dv.id">
                      {{ deckVersionLabel(dv) }}
                    </SelectItem>
                  </SelectGroup>
                </SelectContent>
              </Select>
              <p class="mt-1.5 text-xs text-muted-foreground">
                All imported matches will be assigned to this deck. Confidence scoring compares cards seen in game logs
                against the deck's card list.
              </p>
            </div>

            <Button :disabled="!selectedDeckId" size="lg" @click="startScan">
              <FileUp class="mr-2 size-4" />
              Load Matches
            </Button>
          </CardContent>
        </Card>
      </template>

      <!-- STATE 2: Processing -->
      <template v-if="ready && state === 'processing'">
        <Card>
          <CardContent class="space-y-4 p-6 text-center">
            <Spinner class="mx-auto size-8" />
            <div>
              <p class="text-sm font-medium">{{ stageLabel(scanStage) }}</p>
              <p class="mt-1 text-xs text-muted-foreground">This may take a moment for large histories.</p>
            </div>

            <div v-if="scanTotal > 0" class="mx-auto max-w-sm space-y-1">
              <div class="h-2 overflow-hidden rounded-full bg-muted">
                <div
                  class="h-full rounded-full bg-primary transition-all duration-300"
                  :style="{ width: progressPercent(scanProgress, scanTotal) + '%' }"
                />
              </div>
              <p class="text-xs text-muted-foreground">{{ progressLine(scanProgress, scanTotal) }}</p>
            </div>

            <Button variant="outline" size="sm" @click="cancelScanAndReset">Cancel</Button>
          </CardContent>
        </Card>
      </template>

      <!-- STATE 3: Results -->
      <template v-if="ready && state === 'results'">
        <!-- Deck header -->
        <div class="flex items-center justify-between">
          <div>
            <h2 class="text-xl font-semibold">{{ scanDeckName }}</h2>
            <div class="mt-1 flex items-center gap-4 text-sm text-muted-foreground">
              <span><strong>{{ totalMatches }}</strong> matches</span>
              <template v-if="confidenceStats">
                <span class="text-green-500">{{ confidenceStats.high }} high</span>
                <span class="text-red-500">{{ confidenceStats.low }} low</span>
                <span v-if="confidenceStats.none > 0">{{ confidenceStats.none }} no log</span>
              </template>
            </div>
          </div>
          <Button variant="outline" size="sm" @click="cancelScanAndReset">
            <X class="mr-1.5 size-3.5" />
            Start over
          </Button>
        </div>

        <!-- Filter controls -->
        <div class="flex flex-wrap items-center gap-4">
          <div class="flex items-center gap-2">
            <label for="import-format" class="text-xs text-muted-foreground">Format</label>
            <Select
              :model-value="formatFilter === '' ? ALL_FORMATS : formatFilter"
              @update:model-value="onFormatFilter"
            >
              <SelectTrigger id="import-format" class="h-8 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="ALL_FORMATS">All formats</SelectItem>
                <SelectItem v-for="fmt in availableFormats" :key="fmt" :value="fmt">{{ fmt }}</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div class="flex items-center gap-2">
            <label for="import-sort" class="text-xs text-muted-foreground">Sort by</label>
            <Select v-model="sortBy">
              <SelectTrigger id="import-sort" class="h-8 text-xs">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem v-for="opt in SORT_OPTIONS" :key="opt.value" :value="opt.value">{{ opt.label }}</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div class="flex items-center gap-2">
            <label for="import-confidence" class="text-xs text-muted-foreground">Min. confidence</label>
            <input
              id="import-confidence"
              v-model.number="minConfidence"
              type="range"
              min="0"
              max="1"
              step="0.1"
              class="h-1.5 w-24 cursor-pointer accent-primary"
            />
            <span class="w-10 text-xs text-muted-foreground">{{ minConfidenceLabel(minConfidence) }}</span>
          </div>
        </div>

        <!-- Match table -->
        <ScanResultsTable
          :rows="rows"
          :selected="selectedIds"
          :empty-text="emptyLine(formatFilter, minConfidence)"
          @toggle="toggleSelect"
          @toggle-page="togglePageSelect"
          @show-cards="showCards"
        />

        <!-- Pagination -->
        <div v-if="lastPage > 1" class="flex items-center justify-between">
          <p class="text-xs text-muted-foreground">Page {{ currentPage }} of {{ lastPage }}</p>
          <div class="flex items-center gap-1">
            <Button
              variant="outline"
              size="sm"
              aria-label="Previous page"
              :disabled="currentPage <= 1"
              @click="loadMatches(currentPage - 1)"
            >
              <ChevronLeft class="size-4" />
            </Button>
            <Button
              variant="outline"
              size="sm"
              aria-label="Next page"
              :disabled="currentPage >= lastPage"
              @click="loadMatches(currentPage + 1)"
            >
              <ChevronRight class="size-4" />
            </Button>
          </div>
        </div>
      </template>
    </div>

    <!-- Sticky bottom bar (0.x fixed it to the window; here it sits above the status bar) -->
    <div
      v-if="state === 'results' && (selectedCount > 0 || totalMatches > 0)"
      class="sticky bottom-0 z-10 mt-auto border-t border-border bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/80"
    >
      <div class="mx-auto flex max-w-6xl items-center justify-between gap-4 px-6 py-3">
        <span class="text-sm text-muted-foreground">
          <template v-if="selectedCount > 0">{{ selectedCount }} selected</template>
          <template v-else>{{ totalMatches }} total matches</template>
        </span>
        <div class="flex items-center gap-3">
          <Button v-if="selectedCount > 0" :disabled="importing" @click="importSelected">
            <Spinner v-if="importing" class="mr-2 size-4" />
            {{ importing ? 'Importing...' : `Import ${selectedCount} match(es)` }}
          </Button>
          <Button variant="outline" :disabled="importing || totalMatches === 0" @click="importAllConfirmOpen = true">
            Import All {{ totalMatches }} Matches
          </Button>
        </div>
      </div>
    </div>

    <!-- Import all confirmation -->
    <Dialog v-model:open="importAllConfirmOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Import all matches?</DialogTitle>
          <DialogDescription>
            This will import all <strong>{{ totalMatches }}</strong> matches from the scan and assign them to
            <strong>{{ scanDeckName }}</strong>.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter class="gap-2 sm:gap-0">
          <Button variant="outline" @click="importAllConfirmOpen = false">Cancel</Button>
          <Button @click="importAll">Import {{ totalMatches }} matches</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>

    <!-- Cards modal -->
    <ScanCardsDialog v-model:open="cardsModalOpen" :match="cardsModalMatch" />

    <!-- Delete confirmation modal -->
    <Dialog v-model:open="deleteModalOpen">
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Delete all imported matches?</DialogTitle>
          <DialogDescription>
            This will permanently delete <strong>{{ importedCount }}</strong> imported match(es) and all associated data
            (games, card stats, archetypes). This cannot be undone.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter class="gap-2 sm:gap-0">
          <Button variant="outline" :disabled="deleting" @click="deleteModalOpen = false">Cancel</Button>
          <Button variant="destructive" :disabled="deleting" @click="deleteAllImported">
            <Spinner v-if="deleting" class="mr-2 size-4" />
            {{ deleting ? 'Deleting...' : `Delete ${importedCount} match(es)` }}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
