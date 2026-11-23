<script setup lang="ts">
import { onUnmounted, ref, watch } from 'vue'
import { commands, type CardSearchView } from '@/bindings'
import { Input } from '@/components/ui/input'
import { unwrap } from '@/lib/commands'
import { SEARCH_DEBOUNCE_MS, searchTerm } from '@/lib/manualMatch'

/**
 * Finds cards by name (0.x `CardSearchInput.vue`): from two characters,
 * once typing stops; arrows, Enter and Escape work the list. Only the
 * latest search's answer is shown.
 */
const emit = defineEmits<{ select: [card: CardSearchView] }>()

const query = ref('')
const results = ref<CardSearchView[]>([])
const highlighted = ref(0)
const loading = ref(false)
let timer: ReturnType<typeof setTimeout> | null = null
let requestId = 0

async function search(term: string) {
  const id = ++requestId
  loading.value = true
  try {
    const found = unwrap(await commands.cardSearch(term))
    if (id === requestId) {
      results.value = found
      highlighted.value = 0
    }
  } catch {
    if (id === requestId) results.value = []
  } finally {
    if (id === requestId) loading.value = false
  }
}

watch(query, (value) => {
  if (timer) clearTimeout(timer)
  const term = searchTerm(value)
  if (term === null) {
    requestId++
    results.value = []
    loading.value = false
    return
  }
  timer = setTimeout(() => void search(term), SEARCH_DEBOUNCE_MS)
})

onUnmounted(() => {
  if (timer) clearTimeout(timer)
})

function choose(card: CardSearchView) {
  emit('select', card)
  query.value = ''
  results.value = []
}

function onKeydown(event: KeyboardEvent) {
  if (results.value.length === 0) return
  if (event.key === 'ArrowDown') {
    event.preventDefault()
    highlighted.value = (highlighted.value + 1) % results.value.length
  } else if (event.key === 'ArrowUp') {
    event.preventDefault()
    highlighted.value = (highlighted.value - 1 + results.value.length) % results.value.length
  } else if (event.key === 'Enter') {
    event.preventDefault()
    const card = results.value[highlighted.value]
    if (card) choose(card)
  } else if (event.key === 'Escape') {
    results.value = []
  }
}
</script>

<template>
  <div class="relative">
    <Input v-model="query" placeholder="Search cards by name" autocomplete="off" @keydown="onKeydown" />
    <ul
      v-if="results.length || (searchTerm(query) !== null && !loading)"
      class="absolute z-20 mt-1 max-h-56 w-full overflow-y-auto rounded-md border bg-popover p-1 text-xs shadow-md"
      role="listbox"
    >
      <li
        v-for="(card, i) in results"
        :key="card.catalogId"
        role="option"
        :aria-selected="i === highlighted"
        class="flex cursor-pointer items-center justify-between gap-2 rounded px-2 py-1"
        :class="i === highlighted ? 'bg-accent text-accent-foreground' : ''"
        @mousemove="highlighted = i"
        @mousedown.prevent="choose(card)"
      >
        <span class="truncate">{{ card.name }}</span>
        <span class="shrink-0 text-[10px] text-muted-foreground">{{ card.typeLine }}</span>
      </li>
      <li v-if="results.length === 0" class="px-2 py-1 text-muted-foreground italic">No cards found</li>
    </ul>
  </div>
</template>
