<script setup lang="ts">
// 0.x `MatchSelect.vue`: the format's 10 newest complete matches, or 50
// when searching opponent names.
import { useQuery } from '@tanstack/vue-query'
import { Check, ChevronsUpDown, Search } from 'lucide-vue-next'
import { refDebounced } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import { commands } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Spinner } from '@/components/ui/spinner'
import { SEARCH_DEBOUNCE_MS } from '@/composables/useDebouncedSearch'
import { archetypeKeys } from '@/lib/archetypes'
import { unwrap } from '@/lib/commands'
import { relativeTime } from '@/lib/opponents'

const props = defineProps<{ format: string }>()
const model = defineModel<number | null>({ default: null })

const open = ref(false)
const search = ref('')
const settledSearch = refDebounced(search, SEARCH_DEBOUNCE_MS)
const debounced = computed(() => settledSearch.value.trim())

// A new format starts over (0.x clears the pick).
watch(
  () => props.format,
  (next, previous) => {
    if (previous !== undefined && next !== previous) {
      model.value = null
      search.value = ''
    }
  },
)

const { data, isFetching } = useQuery({
  queryKey: computed(() => archetypeKeys.formMatches(props.format, debounced.value)),
  queryFn: async ({ queryKey }) => unwrap(await commands.createFormMatches(queryKey[2] || null, queryKey[3] || null)),
  enabled: computed(() => props.format !== ''),
})

const matches = computed(() => data.value ?? [])
const now = ref(Date.now())
const selected = computed(() => matches.value.find((match) => match.id === model.value) ?? null)

function pick(id: number) {
  model.value = id
  open.value = false
}

function clear() {
  model.value = null
  search.value = ''
}
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger as-child>
      <Button type="button" variant="outline" role="combobox" :aria-expanded="open" :disabled="!format" class="w-full justify-between">
        <span v-if="selected" class="flex min-w-0 items-center gap-2">
          <span class="truncate">{{ selected.opponent }}</span>
          <span class="text-xs text-muted-foreground">{{ relativeTime(selected.startedAtMs, now) }}</span>
        </span>
        <span v-else class="text-muted-foreground">{{ format ? 'Pick a match' : 'Select a format first' }}</span>
        <ChevronsUpDown class="ml-2 size-4 shrink-0 opacity-50" />
      </Button>
    </PopoverTrigger>
    <PopoverContent class="w-96 p-0" align="start">
      <div class="flex h-9 items-center gap-2 border-b px-3">
        <Search class="size-4 shrink-0 opacity-50" />
        <input
          v-model="search"
          type="text"
          placeholder="Search opponent name..."
          class="flex h-9 w-full bg-transparent py-2 text-sm outline-none placeholder:text-muted-foreground"
        />
        <Spinner v-if="isFetching" class="size-3.5" />
      </div>
      <div class="max-h-64 overflow-y-auto p-1">
        <div v-if="!matches.length" class="px-3 py-6 text-center text-sm text-muted-foreground">
          {{ search ? 'No matches found.' : 'No matches available.' }}
        </div>
        <button
          v-for="match in matches"
          :key="match.id"
          type="button"
          class="flex w-full items-center justify-between gap-2 rounded-sm px-2 py-1.5 text-left text-sm hover:bg-accent hover:text-accent-foreground"
          @click="pick(match.id)"
        >
          <span class="flex min-w-0 items-center gap-2">
            <Check class="size-4 shrink-0" :class="selected?.id === match.id ? 'opacity-100' : 'opacity-0'" />
            <span class="truncate">{{ match.opponent }}</span>
          </span>
          <span class="shrink-0 text-xs text-muted-foreground">{{ relativeTime(match.startedAtMs, now) }}</span>
        </button>
        <button
          v-if="selected"
          type="button"
          class="mt-1 w-full border-t border-border px-2 py-1.5 text-left text-xs text-muted-foreground hover:text-foreground"
          @click="clear"
        >
          Clear selection
        </button>
      </div>
    </PopoverContent>
  </Popover>
</template>
