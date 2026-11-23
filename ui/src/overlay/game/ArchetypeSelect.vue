<script setup lang="ts">
import { Check, ChevronDown } from 'lucide-vue-next'
import { computed, nextTick, ref, watch } from 'vue'
import type { OverlayArchetypeOption } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Skeleton } from '@/components/ui/skeleton'
import {
  NO_ARCHETYPES,
  SEARCH_ARCHETYPES,
  UNKNOWN_ARCHETYPE,
  UNREACHABLE_ARCHETYPE,
  isCurrent,
  moveHighlight,
  optionKey,
  pickerList,
} from '@/lib/overlay'

/**
 * The overlay's archetype picker (0.x `ArchetypeSelect`): a popover with a
 * search, arrow keys, Enter and Escape; the format's archetypes (held ones
 * and, online, unmet ones of its 90-day metagame), then the fallbacks.
 * `options` null is still loading.
 */
const props = defineProps<{
  options: OverlayArchetypeOption[] | null
  currentId: number | null
  currentUuid: string | null
  currentName: string | null
  /** Where the archetype came from, e.g. "guess". */
  sourceLabel?: string | null
  /** No archetype because MyMTGO could not be reached (it is asked again). */
  unreachable?: boolean
}>()

const emit = defineEmits<{ select: [option: OverlayArchetypeOption]; open: [] }>()

const open = ref(false)
const search = ref('')
const highlighted = ref(0)
const listEl = ref<HTMLElement | null>(null)

const navigable = computed(() => pickerList(props.options ?? [], search.value))
const regular = computed(() => navigable.value.filter((option) => !option.isFallback))
const fallbacks = computed(() => navigable.value.filter((option) => option.isFallback))
const loading = computed(() => props.options === null)
const empty = computed(() => !loading.value && navigable.value.length === 0)

watch([search, navigable], () => {
  highlighted.value = 0
})

watch(open, (opened) => {
  if (opened) emit('open')
})

function scrollHighlightedIntoView(): void {
  void nextTick(() => {
    listEl.value?.querySelector<HTMLElement>('[data-highlighted="true"]')?.scrollIntoView({ block: 'nearest' })
  })
}

function onSearchKeydown(event: KeyboardEvent): void {
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault()
    highlighted.value = moveHighlight(highlighted.value, event.key === 'ArrowDown' ? 1 : -1, navigable.value.length)
    scrollHighlightedIntoView()
  } else if (event.key === 'Enter') {
    event.preventDefault()
    const target = navigable.value[highlighted.value]
    if (target) choose(target)
  } else if (event.key === 'Escape') {
    event.preventDefault()
    open.value = false
  }
}

function isHighlighted(option: OverlayArchetypeOption): boolean {
  const at = navigable.value[highlighted.value]
  return at !== undefined && optionKey(at) === optionKey(option)
}

function highlight(option: OverlayArchetypeOption): void {
  const index = navigable.value.findIndex((candidate) => optionKey(candidate) === optionKey(option))
  if (index >= 0) highlighted.value = index
}

function choose(option: OverlayArchetypeOption): void {
  open.value = false
  search.value = ''
  emit('select', option)
}

const current = (option: OverlayArchetypeOption): boolean => isCurrent(option, props.currentId, props.currentUuid)
</script>

<template>
  <Popover v-model:open="open">
    <PopoverTrigger
      class="flex w-full items-center justify-between gap-2 rounded-md border border-border bg-background px-2 py-1.5 text-left text-sm font-semibold"
    >
      <span class="truncate" :class="props.currentName ? '' : 'text-muted-foreground'">
        {{ props.currentName ?? (props.unreachable ? UNREACHABLE_ARCHETYPE : UNKNOWN_ARCHETYPE) }}
      </span>
      <span class="flex shrink-0 items-center gap-1.5">
        <span
          v-if="props.sourceLabel"
          class="rounded bg-muted px-1 py-px text-[10px] font-medium tracking-wide text-muted-foreground uppercase"
          >{{ props.sourceLabel }}</span
        >
        <ChevronDown class="size-4 text-muted-foreground" />
      </span>
    </PopoverTrigger>

    <PopoverContent align="start" side="bottom" class="flex w-[var(--reka-popover-trigger-width)] flex-col gap-1 p-1">
      <Input v-model="search" :placeholder="SEARCH_ARCHETYPES" class="h-7 text-xs" @keydown="onSearchKeydown" />

      <div ref="listEl" class="max-h-56 overflow-y-auto" role="listbox">
        <button
          v-for="option in regular"
          :key="optionKey(option)"
          type="button"
          role="option"
          :aria-selected="current(option)"
          :data-highlighted="isHighlighted(option)"
          class="flex w-full items-center justify-between gap-2 rounded px-2 py-1 text-left text-xs hover:bg-accent"
          :class="isHighlighted(option) ? 'bg-accent' : ''"
          @mousemove="highlight(option)"
          @click="choose(option)"
        >
          <span class="flex min-w-0 items-center gap-1">
            <Check v-if="current(option)" class="size-3 shrink-0" />
            <span class="truncate">{{ option.name }}</span>
          </span>
          <ManaColors v-if="option.colors.length" :colors="option.colors" class="shrink-0" />
        </button>

        <button
          v-for="option in fallbacks"
          :key="optionKey(option)"
          type="button"
          role="option"
          :aria-selected="current(option)"
          :data-highlighted="isHighlighted(option)"
          class="flex w-full items-center gap-1 rounded px-2 py-1 text-left text-xs text-muted-foreground hover:bg-accent"
          :class="isHighlighted(option) ? 'bg-accent' : ''"
          @mousemove="highlight(option)"
          @click="choose(option)"
        >
          <Check v-if="current(option)" class="size-3 shrink-0" />
          <span class="truncate">{{ option.name }}</span>
        </button>

        <div v-if="loading" class="flex flex-col gap-1 p-1" aria-label="Loading archetypes">
          <Skeleton v-for="row in 5" :key="row" class="h-5 w-full" />
        </div>

        <p v-else-if="empty" class="px-2 py-4 text-center text-xs text-muted-foreground">{{ NO_ARCHETYPES }}</p>
      </div>
    </PopoverContent>
  </Popover>
</template>
