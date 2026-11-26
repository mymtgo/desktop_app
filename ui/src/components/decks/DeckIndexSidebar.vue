<script setup lang="ts">
import { History, LayoutGrid, Plus, TriangleAlert } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { DeckArchetypeOptionView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import ArchetypePicker from '@/components/matches/ArchetypePicker.vue'
import { Label } from '@/components/ui/label'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { useArchetypeOptions } from '@/composables/useOpponentArchetype'
import { acceptsDrag, hasDeckDrag, readDeckDrag } from '@/lib/deckDrag'
import { archetypeRate, archetypeRowTitle, deckCount, rateTone, visibleArchetypes } from '@/lib/deckIndex'
import { cn } from '@/lib/utils'

/**
 * The deck index sidebar (0.x `DeckIndexSidebar`): "Other archetype…", "Recent decks", "Unclassified", "All decks"
 * (neither recent nor an archetype) pinned, then the archetype rows, the
 * only part that scrolls; the view switches at the foot.
 */
const props = withDefaults(
  defineProps<{
    /** A raw format code, or `''` for every format (search and format are in the layout's top bar). */
    format: string
    /** Whether the "recently played" filter is applied. */
    recent: boolean
    /** Asked for but not applied: no deck has a complete match yet. */
    recentUnavailable?: boolean
    compactCards: boolean
    showArchived: boolean
    /** `none`, an archetype id, or null. */
    archetype?: string | null
    archetypeOptions?: DeckArchetypeOptionView[]
    unclassifiedCount?: number
    /** The selected decks, assigned by "Other archetype…". */
    selectedCount?: number
    /** The selection's shared format, or null. */
    selectionFormat?: string | null
    /** The format of the decks being dragged; null when no drag is on. */
    dragFormat?: string | null
  }>(),
  {
    recentUnavailable: false,
    archetype: null,
    archetypeOptions: () => [],
    unclassifiedCount: 0,
    selectedCount: 0,
    selectionFormat: null,
    dragFormat: null,
  },
)

const emit = defineEmits<{
  'update:recent': [value: boolean]
  'update:compactCards': [value: boolean]
  'update:showArchived': [value: boolean]
  'update:archetype': [value: string]
  /** "All decks": clear the recent and archetype filters. */
  showAll: []
  /** Assign decks (none given: the selection) to an archetype, or clear it (null). */
  assign: [deckIds: number[], archetypeId: number | null]
}>()

// Archetypes without a match stay hidden until asked for; remembered per viewer.
const SHOW_EMPTY_KEY = 'mymtgo.deckIndex.showEmptyArchetypes'

function readShowEmpty(): boolean {
  try {
    return localStorage.getItem(SHOW_EMPTY_KEY) === '1'
  } catch {
    return false
  }
}

const showEmpty = ref(readShowEmpty())

function toggleShowEmpty() {
  showEmpty.value = !showEmpty.value
  try {
    localStorage.setItem(SHOW_EMPTY_KEY, showEmpty.value ? '1' : '0')
  } catch {
    // Storage can be blocked; the choice then lasts until the page closes.
  }
}

// A deck drag shows every row, so a deck can be dropped on an archetype without matches.
const dragging = computed(() => props.dragFormat !== null)
const visible = computed(() =>
  visibleArchetypes(props.archetypeOptions, { showAll: showEmpty.value || dragging.value, selected: props.archetype }),
)

// Drop targets (0.x): `dragover` sees only the payload's type, so the
// highlight and the accept key off it; a row of another format refuses.
const dropTarget = ref<string | null>(null)

function onDragOver(event: DragEvent, key: string, rowFormat: string | null = null) {
  if (!hasDeckDrag(event.dataTransfer)) return
  if (!acceptsDrag(props.dragFormat, rowFormat)) {
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'none'
    return
  }
  event.preventDefault()
  if (event.dataTransfer) event.dataTransfer.dropEffect = 'move'
  dropTarget.value = key
}

function onDragLeave(event: DragEvent, key: string) {
  const next = event.relatedTarget as Node | null
  if (next && (event.currentTarget as HTMLElement).contains(next)) return
  if (dropTarget.value === key) dropTarget.value = null
}

function onDrop(event: DragEvent, archetypeId: number | null, rowFormat: string | null = null) {
  dropTarget.value = null
  const ids = readDeckDrag(event.dataTransfer)
  if (ids.length === 0 || !acceptsDrag(props.dragFormat, rowFormat)) return
  event.preventDefault()
  emit('assign', ids, archetypeId)
}

// "Other archetype…": a drop keeps the dragged decks for the picker; a
// click assigns the selection.
const otherOpen = ref(false)
const pendingIds = ref<number[] | null>(null)
const otherFormat = computed(() => (pendingIds.value ? props.dragFormat : props.selectionFormat))
const { data: pickerOptions } = useArchetypeOptions(
  otherFormat,
  computed(() => otherOpen.value),
)

function onDropOther(event: DragEvent) {
  dropTarget.value = null
  const ids = readDeckDrag(event.dataTransfer)
  if (ids.length === 0) return
  event.preventDefault()
  pendingIds.value = ids
  otherOpen.value = true
}

function onOtherOpenChange(open: boolean) {
  otherOpen.value = open
  if (!open) pendingIds.value = null
}

function onOtherPick(archetypeId: number) {
  emit('assign', pendingIds.value ?? [], archetypeId)
  onOtherOpenChange(false)
}

const dropClass = 'border-primary/60 bg-primary/10 text-foreground'

const rowClass = 'flex w-full cursor-pointer items-center gap-2 rounded px-2.5 py-1.5 text-left text-sm transition-colors'
</script>

<template>
  <div class="flex h-full flex-col border-r border-black/80 bg-muted/20">
    <!-- Middle: the filter rows stay put; only the archetype rows scroll. -->
    <section class="flex min-h-0 flex-1 flex-col border-b border-b-black/60">
      <div class="flex shrink-0 flex-col gap-1 px-3 pt-3">
        <h3 class="px-2.5 text-[10px] font-semibold tracking-wider text-muted-foreground/70 uppercase">
          Archetype
        </h3>

        <Popover :open="otherOpen" @update:open="onOtherOpenChange">
          <PopoverTrigger as-child>
            <button
              type="button"
              :class="cn(rowClass, 'nav-item-inactive', dropTarget === 'other' && dropClass)"
              :title="selectedCount === 0 ? 'Select decks, or drop one here' : `Assign ${selectedCount} selected`"
              @dragover="onDragOver($event, 'other')"
              @dragleave="onDragLeave($event, 'other')"
              @drop="onDropOther"
            >
              <Plus class="size-3.5 shrink-0" />
              <span class="flex-1">Other archetype…</span>
            </button>
          </PopoverTrigger>
          <PopoverContent side="right" align="start" class="w-72 p-2">
            <p v-if="selectedCount === 0 && !pendingIds" class="px-2 py-3 text-sm text-muted-foreground">
              Select one or two decks or drag a deck to assign a new archetype
            </p>
            <ArchetypePicker v-else :archetypes="pickerOptions" :format="otherFormat" autofocus @select="onOtherPick" />
          </PopoverContent>
        </Popover>

        <button
          type="button"
          :class="
            cn(
              rowClass,
              recent && !archetype ? 'nav-item-active' : 'nav-item-inactive',
              'disabled:cursor-not-allowed disabled:opacity-50',
            )
          "
          :aria-pressed="recent && !archetype"
          :disabled="recentUnavailable"
          :title="
            recentUnavailable
              ? 'No decks yet, so every deck is shown'
              : 'Your 8 most recently played or changed decks'
          "
          @click="emit('update:recent', !(recent && !archetype))"
        >
          <History class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="flex-1">Recent decks</span>
        </button>

        <button
          type="button"
          :class="cn(rowClass, archetype === 'none' ? 'nav-item-active' : 'nav-item-inactive', dropTarget === 'none' && dropClass)"
          :aria-pressed="archetype === 'none'"
          @click="emit('update:archetype', 'none')"
          @dragover="onDragOver($event, 'none')"
          @dragleave="onDragLeave($event, 'none')"
          @drop="onDrop($event, null)"
        >
          <TriangleAlert :class="cn('size-3.5 shrink-0', unclassifiedCount === 0 ? 'text-muted-foreground' : 'text-warning')" />
          <span class="flex-1">Unclassified</span>
          <span
            :class="cn('text-xs font-medium tabular-nums', unclassifiedCount === 0 ? 'text-muted-foreground' : 'text-warning')"
          >
            {{ unclassifiedCount }}
          </span>
        </button>

        <button
          type="button"
          :class="cn(rowClass, !recent && !archetype ? 'nav-item-active' : 'nav-item-inactive')"
          :aria-pressed="!recent && !archetype"
          @click="emit('showAll')"
        >
          <LayoutGrid class="size-3.5 shrink-0 text-muted-foreground" />
          <span class="flex-1">All decks</span>
        </button>

        <div class="my-1 border-t border-border/60" />
      </div>

      <div class="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto px-3 pb-3">
        <button
          v-for="option in visible.shown"
          :key="option.id"
          type="button"
          :class="
            cn(
              rowClass,
              archetype === String(option.id) ? 'nav-item-active' : 'nav-item-inactive',
              dropTarget === String(option.id) && dropClass,
              dragFormat !== null && !acceptsDrag(dragFormat, option.format) && 'opacity-40',
            )
          "
          :aria-pressed="archetype === String(option.id)"
          :title="archetypeRowTitle(option)"
          @click="emit('update:archetype', String(option.id))"
          @dragover="onDragOver($event, String(option.id), option.format)"
          @dragleave="onDragLeave($event, String(option.id))"
          @drop="onDrop($event, option.id, option.format)"
        >
          <span class="flex min-w-0 flex-1 flex-col gap-0.5">
            <span class="flex items-center gap-2">
              <span class="flex min-w-0 flex-1 items-center gap-1.5">
                <span class="truncate">{{ option.name }}</span>
                <span
                  v-if="option.isCustom"
                  class="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wide text-muted-foreground uppercase"
                >
                  Custom
                </span>
              </span>
              <span :class="cn('shrink-0 tabular-nums', rateTone(option.record))">{{ archetypeRate(option.record) }}</span>
            </span>
            <span class="flex items-center gap-2 text-xs">
              <span class="min-w-0 truncate text-[10px] tracking-wide text-muted-foreground uppercase">
                <!-- Same-named archetypes exist per format; only ambiguous with every format shown. -->
                <template v-if="format === '' && option.format">{{ option.format }} &middot; </template>
                {{ deckCount(option.deckCount) }}
              </span>
              <!-- After the text, so the text lines up whatever the colour count. -->
              <ManaColors v-if="option.colors.length" :colors="option.colors" size="xs" class="shrink-0" />
              <span v-if="option.record.total > 0" class="ml-auto shrink-0 text-muted-foreground tabular-nums">
                {{ option.record.label }}
              </span>
            </span>
          </span>
        </button>
        <button
          v-if="visible.hiddenCount > 0 && !dragging"
          type="button"
          class="cursor-pointer rounded px-2.5 py-1.5 text-left text-xs text-muted-foreground transition-colors hover:text-foreground"
          @click="toggleShowEmpty"
        >
          {{ showEmpty ? 'Hide archetypes without matches' : `Show ${visible.hiddenCount} without matches` }}
        </button>
      </div>
    </section>

    <!-- Pinned bottom: view -->
    <section class="flex flex-col gap-2 border-t border-white/5 px-3 py-3">
      <h3 class="px-2.5 text-[10px] font-semibold tracking-wider text-muted-foreground/70 uppercase">View</h3>
      <div class="flex items-center justify-between px-2.5">
        <Label for="compact-cards" class="cursor-pointer text-xs">Compact cards</Label>
        <Switch id="compact-cards" :model-value="compactCards" @update:model-value="(value) => emit('update:compactCards', value)" />
      </div>
      <div class="flex items-center justify-between px-2.5">
        <Label for="show-archived" class="cursor-pointer text-xs">Show archived</Label>
        <Switch id="show-archived" :model-value="showArchived" @update:model-value="(value) => emit('update:showArchived', value)" />
      </div>
    </section>
  </div>
</template>
