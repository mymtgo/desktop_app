<script setup lang="ts">
// One archetype (archetypes spec section 10, laid out per issue #326): the
// side menu of archetypes on the left (0.x's sidebar); the header with the
// archetype's name, the record against it, its metagame share and its
// actions; then the tabs Decklist (typical lists with a version picker,
// cards seen), Matches (the player's decks against it, the matches) and
// Sideboard guides (spec section 21.4). Tab, timeframe and list version are
// in the route.
import { usePlatformContext } from '@/composables/usePlatformContext'
import { useQueryClient } from '@tanstack/vue-query'
import { ChevronLeft, Shapes } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { commands, type MatchListQuery } from '@/bindings'
import MergeArchetypeDialog from '@/components/archetypes/MergeArchetypeDialog.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import ArchetypeGuidesBlock from '@/components/guides/ArchetypeGuidesBlock.vue'
import ArchetypeSidebar from '@/components/metagame/ArchetypeSidebar.vue'
import CardsSeen from '@/components/metagame/CardsSeen.vue'
import TimeframeSwitch from '@/components/metagame/TimeframeSwitch.vue'
import TypicalLists from '@/components/metagame/TypicalLists.vue'
import MatchesTable from '@/components/MatchesTable.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import PagedNav from '@/components/PagedNav.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { useToast } from '@/composables/useToast'
import { MANAGE_PATH, archetypeEditPath, mergedNotice, sourceBadge } from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'
import {
  ARCHETYPE_TABS,
  archetypeLocation,
  arenaSelection,
  archetypeTab,
  chosenTimeframe,
  differenceText,
  differenceTone,
  globalNotice,
  globalText,
  metagameKeys,
  metagameLocation,
  searchQuery,
  readRememberedFormat,
  readRememberedTimeframe,
  shareText,
  useArchetypeDecklists,
  useArchetypeDetail,
  useMetagame,
  writeRememberedTimeframe,
  type ArchetypeTab,
} from '@/lib/metagame'
import { useMatchesPage } from '@/lib/queries'

const route = useRoute()
const router = useRouter()
const queryClient = useQueryClient()
const { add } = useToast()

const id = computed(() => Number(route.params.id) || 0)
// The Metagame page's timeframe carries over (and back): issue #326.
const timeframe = computed(() => chosenTimeframe(route.query.timeframe, readRememberedTimeframe(window.localStorage)))
const { data: detail, error, isPending } = useArchetypeDetail(id, timeframe)
// Switching archetypes in the side menu: the previous one's detail stays as
// placeholder data until the new one arrives. Show the loading state then, so
// the header and its actions never belong to another archetype.
const loading = computed(() => isPending.value || (detail.value != null && detail.value.id !== id.value))
const { data: decklists, isPending: decklistsPending } = useArchetypeDecklists(id)

const chosenTimeframeModel = computed({
  get: () => timeframe.value,
  set: (key: string) => {
    writeRememberedTimeframe(window.localStorage, key)
    void router.push({ query: { ...route.query, timeframe: key } })
  },
})

const tab = computed<ArchetypeTab>(() => archetypeTab(route.params.tab))
const tabModel = computed({
  get: () => tab.value,
  set: (next: string | number) =>
    void router.push({ name: 'archetype', params: { id: String(id.value), tab: String(next) }, query: route.query }),
})

const version = computed(() => (typeof route.query.version === 'string' ? route.query.version : null))
function showVersion(key: string) {
  void router.push({ query: { ...route.query, version: key } })
}

const { platform } = usePlatformContext()
const backToMetagame = computed(() =>
  metagameLocation(readRememberedFormat(window.localStorage) ?? detail.value?.format ?? null, timeframe.value, searchQuery(route.query.q), arenaSelection(platform.value, route.query.month, route.query.rank)),
)

// The metagame share: this archetype's tile on its format's Metagame page.
const format = computed(() => (loading.value ? null : (detail.value?.format ?? null)))
const { data: metagame } = useMetagame(
  format,
  timeframe,
  computed(() => format.value !== null),
)
const notice = computed(() => (metagame.value && tile.value ? globalNotice(metagame.value.globalState) : null))
const tile = computed(() => metagame.value?.tiles.find((each) => each.archetypeId === id.value) ?? null)

// The live decklists change which cards seen are typical: once they are
// cached, the detail reads them.
watch(
  () => decklists.value?.state,
  (state) => {
    if (state === 'ready') void queryClient.invalidateQueries({ queryKey: metagameKeys.detail(id.value, timeframe.value) })
  },
)

// The shared matches table, scoped to matches against this archetype.
const matchesPage = ref(1)
watch(id, () => (matchesPage.value = 1))
const matchQuery = computed<MatchListQuery>(() => ({
  from: null,
  to: null,
  result: null,
  kind: null,
  archetype: String(id.value),
  sort: 'started_at',
  sortDir: 'desc',
  page: matchesPage.value,
}))
const { data: matches } = useMatchesPage(matchQuery)

const badge = computed(() => (detail.value ? sourceBadge(detail.value.source, detail.value.retired, detail.value.offlineHeld) : null))
const canMerge = computed(() => detail.value !== null && detail.value !== undefined && detail.value.source !== 'fallback')

async function refresh() {
  await queryClient.invalidateQueries()
}

const mergeOpen = ref(false)

async function unmerge() {
  if (!detail.value) return
  try {
    add({ type: 'success', title: unwrap(await commands.unmergeArchetype(detail.value.id)), message: '' })
    await refresh()
  } catch (failure) {
    add({ type: 'error', title: 'Could not unmerge', message: errorMessage(failure) })
  }
}

const deleteOpen = ref(false)
async function confirmDelete() {
  if (!detail.value) return
  try {
    unwrap(await commands.deleteArchetype(detail.value.id))
    deleteOpen.value = false
    await router.push(MANAGE_PATH)
    await refresh()
  } catch (failure) {
    add({ type: 'error', title: 'Could not delete', message: errorMessage(failure) })
  }
}

// The global share fills in when the archetypes worker has it
// (`onMetagameSharesReady`).
</script>

<template>
  <div class="flex min-h-0 flex-1">
    <div class="w-72 shrink-0">
      <ArchetypeSidebar
        :selected-id="id"
        :archetype-format="loading ? null : (detail?.format ?? null)"
        :tab="tab"
        :timeframe="timeframe"
        :metagame-search="searchQuery(route.query.q)"
      />
    </div>

    <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3 lg:p-4">
      <RouterLink
        :to="backToMetagame"
        class="inline-flex items-center gap-1 self-start text-sm text-muted-foreground transition-colors hover:text-foreground"
      >
        <ChevronLeft class="size-4" />
        Metagame
      </RouterLink>

      <p v-if="error" class="text-sm text-destructive">Could not load the archetype: {{ error.message }}</p>

      <template v-else-if="loading">
        <Skeleton class="h-10 w-72" />
        <Skeleton class="h-40 w-full" />
      </template>

      <div v-else-if="!detail" class="flex flex-col items-center gap-2 py-16 text-center">
        <Shapes class="size-10 text-muted-foreground/40" />
        <p class="font-medium">This archetype no longer exists.</p>
        <RouterLink :to="MANAGE_PATH" class="text-sm text-muted-foreground hover:underline">Manage archetypes</RouterLink>
      </div>

      <template v-else>
        <header class="flex flex-col gap-3 border-b pb-4">
          <div class="flex flex-wrap items-start justify-between gap-3">
            <div class="flex flex-col gap-1">
              <h1 class="flex flex-wrap items-center gap-2 text-lg font-semibold">
                <ManaColors v-if="detail.colors.length" :colors="detail.colors" />
                {{ detail.name }}
                <Badge v-if="badge" variant="outline">{{ badge }}</Badge>
                <Badge v-if="detail.incomplete" variant="outline">Incomplete</Badge>
              </h1>
              <p v-if="detail.formatLabel" class="text-sm text-muted-foreground">
                {{ detail.formatLabel }} · {{ detail.timeframeLabel }}
              </p>
              <p v-if="detail.mergedInto" class="text-sm">
                {{ mergedNotice(detail.mergedInto.name).before
                }}<RouterLink :to="archetypeLocation(detail.mergedInto.id, tab, timeframe)" class="font-medium hover:underline">{{
                  detail.mergedInto.name
                }}</RouterLink
                >{{ mergedNotice(detail.mergedInto.name).after }}
              </p>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <TimeframeSwitch v-model="chosenTimeframeModel" :timeframes="detail.timeframes" />
              <Button v-if="detail.source === 'manual'" size="sm" variant="outline" as-child>
                <RouterLink :to="archetypeEditPath(detail.id)">Edit</RouterLink>
              </Button>
              <Button v-else-if="detail.source === 'commander'" size="sm" variant="outline" as-child>
                <RouterLink :to="archetypeEditPath(detail.id)">Rename</RouterLink>
              </Button>
              <Button v-if="canMerge && !detail.mergedInto" size="sm" variant="outline" @click="mergeOpen = true">
                Merge
              </Button>
              <Button v-if="detail.mergedInto" size="sm" variant="outline" @click="unmerge">Unmerge</Button>
              <Button v-if="detail.source === 'manual'" size="sm" variant="destructive" @click="deleteOpen = true">
                Delete
              </Button>
            </div>
          </div>

          <dl class="grid grid-cols-2 gap-3 text-xs sm:grid-cols-4">
            <div class="flex flex-col">
              <dt class="text-muted-foreground">Your win rate</dt>
              <dd class="text-lg font-semibold tabular-nums">{{ detail.matchWinRate }}%</dd>
              <dd class="text-muted-foreground tabular-nums">
                {{ detail.matchRecord }} · {{ detail.matches }} {{ detail.matches === 1 ? 'match' : 'matches' }}
              </dd>
            </div>
            <div class="flex flex-col">
              <dt class="text-muted-foreground">Your share</dt>
              <dd class="text-lg font-semibold tabular-nums">{{ tile ? shareText(tile.share ?? 0) : '-' }}</dd>
            </div>
            <div class="flex flex-col">
              <dt class="text-muted-foreground">Global share</dt>
              <dd class="text-lg font-semibold tabular-nums">
                {{ tile && metagame ? globalText(tile, metagame.globalState) : '-' }}
              </dd>
            </div>
            <div class="flex flex-col">
              <dt class="text-muted-foreground">Difference</dt>
              <dd
                class="text-lg font-semibold tabular-nums"
                :class="{
                  'text-success': tile && metagame && differenceTone(tile, metagame.globalState) === 'up',
                  'text-destructive': tile && metagame && differenceTone(tile, metagame.globalState) === 'down',
                }"
              >
                {{ tile && metagame ? differenceText(tile, metagame.globalState) : '-' }}
              </dd>
            </div>
          </dl>
          <p v-if="notice" class="text-xs text-muted-foreground">{{ notice }}</p>
        </header>

        <Tabs v-model="tabModel" class="flex flex-col gap-4">
          <TabsList>
            <TabsTrigger v-for="each in ARCHETYPE_TABS" :key="each.key" :value="each.key">{{ each.label }}</TabsTrigger>
          </TabsList>

          <TabsContent value="decklist" class="flex flex-col gap-4">
            <!-- The list is its own card: the tab already says "Decklist". -->
            <TypicalLists
              :archetype-id="detail.id"
              :archetype-name="detail.name"
              :source="detail.source"
              :decklists="decklists"
              :loading="decklistsPending"
              :stored="detail.lists"
              :version="version"
              @update:version="showVersion"
              @changed="refresh"
            />
            <Card>
              <CardHeader><CardTitle class="text-sm">Cards seen</CardTitle></CardHeader>
              <CardContent><CardsSeen :cards="detail.cardsSeen" /></CardContent>
            </Card>
          </TabsContent>

          <TabsContent value="matches" class="flex flex-col gap-4">
            <Card>
              <CardHeader><CardTitle class="text-sm">Your decks against it</CardTitle></CardHeader>
              <CardContent class="px-0">
                <p v-if="detail.decks.length === 0" class="px-6 text-sm text-muted-foreground">
                  No deck has played against it.
                </p>
                <Table v-else>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Deck</TableHead>
                      <TableHead class="text-right">Matches</TableHead>
                      <TableHead class="text-right">Match win rate</TableHead>
                      <TableHead class="text-right">Game win rate</TableHead>
                      <TableHead class="text-right">On the play</TableHead>
                      <TableHead class="text-right">On the draw</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    <TableRow v-for="deck in detail.decks" :key="deck.deckId">
                      <TableCell>
                        <RouterLink :to="`/decks/${deck.deckId}`" class="flex items-center gap-1.5 font-medium hover:underline">
                          <ManaColors v-if="deck.colors.length" :colors="deck.colors" size="xs" />
                          {{ deck.name }}
                        </RouterLink>
                      </TableCell>
                      <TableCell class="text-right tabular-nums">{{ deck.matches }}</TableCell>
                      <TableCell class="text-right tabular-nums">{{ deck.matchWinRate }}% ({{ deck.matchRecord }})</TableCell>
                      <TableCell class="text-right tabular-nums">{{ deck.gameWinRate }}% ({{ deck.gameRecord }})</TableCell>
                      <TableCell class="text-right tabular-nums">{{ deck.otpWinRate }}% ({{ deck.otpRecord }})</TableCell>
                      <TableCell class="text-right tabular-nums">{{ deck.otdWinRate }}% ({{ deck.otdRecord }})</TableCell>
                    </TableRow>
                  </TableBody>
                </Table>
              </CardContent>
            </Card>

            <Card class="gap-0 overflow-hidden p-0">
              <CardHeader class="py-4"><CardTitle class="text-sm">Matches</CardTitle></CardHeader>
              <CardContent class="px-0">
                <p v-if="!matches || matches.rows.length === 0" class="py-8 text-center text-sm text-muted-foreground">
                  No matches against it
                </p>
                <MatchesTable v-else :rows="matches.rows" />
              </CardContent>
              <PagedNav
                v-if="matches && matches.total > matches.perPage"
                class="justify-end px-2 py-2"
                :per-page="matches.perPage"
                :total="matches.total"
                :page="matches.page"
                @update:page="(page: number) => (matchesPage = page)"
              />
            </Card>
          </TabsContent>

          <TabsContent value="sideboard-guides">
            <ArchetypeGuidesBlock :archetype-id="detail.id" :retired="detail.retired" />
          </TabsContent>
        </Tabs>

        <MergeArchetypeDialog
          v-if="canMerge"
          v-model:open="mergeOpen"
          :archetype="{ id: detail.id, name: detail.name }"
          @merged="refresh"
        />

        <Dialog v-model:open="deleteOpen">
          <DialogContent class="max-w-sm">
            <DialogHeader>
              <DialogTitle>Delete Archetype</DialogTitle>
              <DialogDescription>Are you sure you want to delete "{{ detail.name }}"? This cannot be undone.</DialogDescription>
            </DialogHeader>
            <DialogFooter>
              <Button variant="outline" size="sm" @click="deleteOpen = false">Cancel</Button>
              <Button variant="destructive" size="sm" @click="confirmDelete">Delete</Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </template>
    </div>
  </div>
</template>
