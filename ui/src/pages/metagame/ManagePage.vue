<script setup lang="ts">
// The manage screen (archetypes spec section 11; 0.x archetype index): the
// held archetypes, 25 a page, searched and filtered by format and source
// (Custom, Server) in the URL,
// the filters remembered in localStorage (0.x kept them in the session).
// Rename (edit), merge, unmerge and delete happen here.
import { useQueryClient } from '@tanstack/vue-query'
import { MoreHorizontal, Plus, Search, Shapes } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { commands, type ManageRowView } from '@/bindings'
import SegmentedControl from '@/components/SegmentedControl.vue'
import MergeArchetypeDialog from '@/components/archetypes/MergeArchetypeDialog.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import PagedNav from '@/components/PagedNav.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useDebouncedSearch } from '@/composables/useDebouncedSearch'
import { useToast } from '@/composables/useToast'
import {
  archetypeDetailPath,
  archetypeEditPath,
  colorLetters,
  createArchetypeRoute,
  manageFromUrl,
  manageUrlQuery,
  readRememberedFilters,
  sourceBadge,
  useManageArchetypes,
  writeRememberedFilters,
  type ManageSource,
  type ManageState,
} from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'

const route = useRoute()
const router = useRouter()
const queryClient = useQueryClient()
const { add } = useToast()

const resolved = computed(() => manageFromUrl(route.query, readRememberedFilters(window.localStorage)))
const state = computed<ManageState>(() => resolved.value.state)

watch(
  () => resolved.value.remember,
  (filters) => {
    if (filters) writeRememberedFilters(window.localStorage, filters)
  },
  { immediate: true },
)

const query = computed(() => ({
  search: state.value.search || null,
  format: state.value.format || null,
  source: state.value.source || null,
  page: state.value.page,
}))
const { data, error, isPending } = useManageArchetypes(query)

const sourceOptions = [
  { value: '', label: 'All' },
  { value: 'manual', label: 'Custom' },
  { value: 'downloaded', label: 'Server' },
  { value: 'commander', label: 'Commander' },
]

function setState(next: ManageState) {
  void router.push({ query: manageUrlQuery(next) })
}

function change(next: Partial<ManageState>) {
  setState({ ...state.value, ...next, page: 1 })
}

const search = useDebouncedSearch({
  current: () => state.value.search,
  apply: (next) => change({ search: next }),
})

const lastPage = computed(() => (data.value ? Math.max(1, Math.ceil(data.value.total / data.value.perPage)) : 1))

// Every page that names archetypes may show the edited one (T7 review m4).
async function refresh() {
  await queryClient.invalidateQueries()
}

const merging = ref<ManageRowView | null>(null)
const mergeOpen = computed({
  get: () => merging.value !== null,
  set: (open: boolean) => {
    if (!open) merging.value = null
  },
})

async function unmerge(row: ManageRowView) {
  try {
    add({ type: 'success', title: unwrap(await commands.unmergeArchetype(row.id)), message: '' })
    await refresh()
  } catch (failure) {
    add({ type: 'error', title: 'Could not unmerge', message: errorMessage(failure) })
  }
}

const deleting = ref<ManageRowView | null>(null)
const deleteOpen = computed({
  get: () => deleting.value !== null,
  set: (open: boolean) => {
    if (!open) deleting.value = null
  },
})

async function confirmDelete() {
  const row = deleting.value
  if (!row) return
  try {
    unwrap(await commands.deleteArchetype(row.id))
    deleting.value = null
    await refresh()
  } catch (failure) {
    add({ type: 'error', title: 'Could not delete', message: errorMessage(failure) })
  }
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex items-center justify-between gap-3">
      <h1 class="text-base font-semibold tracking-tight">Manage archetypes</h1>
      <Button size="sm" as-child>
        <RouterLink :to="createArchetypeRoute()">
          <Plus class="mr-1.5 size-4" />
          Create archetype
        </RouterLink>
      </Button>
    </div>

    <div class="flex flex-wrap items-center gap-3">
      <div class="relative">
        <Search class="absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input v-model="search" placeholder="Search archetypes..." aria-label="Search archetypes" class="h-8 w-56 pl-7" />
      </div>
      <SegmentedControl
        :model-value="state.source"
        :options="sourceOptions"
        @update:model-value="(value) => change({ source: value as ManageSource })"
      />
      <SegmentedControl
        class="ml-auto"
        :model-value="state.format"
        :options="[{ value: '', label: 'All' }, ...(data?.formats ?? []).map((format) => ({ value: format.key, label: format.label }))]"
        @update:model-value="(value) => change({ format: value })"
      />
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load archetypes: {{ error.message }}</p>

    <template v-else-if="isPending || !data">
      <Skeleton v-for="n in 6" :key="n" class="h-12 w-full" />
    </template>

    <Card v-else class="overflow-hidden">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Archetype</TableHead>
            <TableHead>Format</TableHead>
            <TableHead>Colors</TableHead>
            <TableHead class="text-right">Matches faced</TableHead>
            <TableHead class="text-right">Variants</TableHead>
            <TableHead>Merged into</TableHead>
            <TableHead class="w-0"></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="data.rows.length === 0">
            <TableCell colspan="7" class="py-12 text-center">
              <div class="flex flex-col items-center gap-2">
                <Shapes class="size-10 text-muted-foreground/40" />
                <p class="font-medium">No archetypes found</p>
              </div>
            </TableCell>
          </TableRow>
          <TableRow v-for="row in data.rows" :key="row.id">
            <TableCell>
              <div class="flex items-center gap-2">
                <RouterLink :to="archetypeDetailPath(row.id)" class="font-medium hover:underline">{{ row.name }}</RouterLink>
                <Badge v-if="sourceBadge(row.source, row.retired, row.offlineHeld)" variant="outline">{{ sourceBadge(row.source, row.retired, row.offlineHeld) }}</Badge>
                <Badge v-if="row.incomplete" variant="outline">Incomplete</Badge>
              </div>
            </TableCell>
            <TableCell class="text-sm text-muted-foreground">{{ row.formatLabel ?? '' }}</TableCell>
            <TableCell><ManaColors v-if="row.colors.length" :colors="colorLetters(row.colors.join(','))" /></TableCell>
            <TableCell class="text-right tabular-nums">{{ row.matchesFaced }}</TableCell>
            <TableCell class="text-right tabular-nums">{{ row.variants }}</TableCell>
            <TableCell class="text-sm">
              <RouterLink v-if="row.mergedInto" :to="archetypeDetailPath(row.mergedInto.id)" class="hover:underline">
                {{ row.mergedInto.name }}
              </RouterLink>
            </TableCell>
            <TableCell>
              <DropdownMenu v-if="row.source !== 'fallback'">
                <DropdownMenuTrigger as-child>
                  <Button variant="ghost" size="sm" :aria-label="`Actions for ${row.name}`">
                    <MoreHorizontal class="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem v-if="row.source === 'manual'" @select="router.push(archetypeEditPath(row.id))">Edit</DropdownMenuItem>
                  <DropdownMenuItem v-else-if="row.source === 'commander'" @select="router.push(archetypeEditPath(row.id))">Rename</DropdownMenuItem>
                  <DropdownMenuItem v-if="!row.mergedInto" @select="merging = row">Merge</DropdownMenuItem>
                  <DropdownMenuItem v-else @select="unmerge(row)">Unmerge</DropdownMenuItem>
                  <DropdownMenuItem v-if="row.source === 'manual'" class="text-destructive" @select="deleting = row">Delete</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>

      <PagedNav
        v-if="lastPage > 1"
        class="justify-end px-2 py-2"
        :per-page="data.perPage"
        :total="data.total"
        :page="data.page"
        @update:page="(page) => setState({ ...state, page })"
      />
    </Card>

    <MergeArchetypeDialog v-if="merging" v-model:open="mergeOpen" :archetype="merging" @merged="refresh" />

    <Dialog v-model:open="deleteOpen">
      <DialogContent class="max-w-sm">
        <DialogHeader>
          <DialogTitle>Delete Archetype</DialogTitle>
          <DialogDescription>Are you sure you want to delete "{{ deleting?.name }}"? This cannot be undone.</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" size="sm" @click="deleting = null">Cancel</Button>
          <Button variant="destructive" size="sm" @click="confirmDelete">Delete</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
</template>
