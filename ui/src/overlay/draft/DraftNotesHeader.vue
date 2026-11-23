<script setup lang="ts">
import { Check, ChevronLeft, ChevronRight, Loader2 } from 'lucide-vue-next'
import { computed } from 'vue'
import type { DraftNotePickView } from '@/bindings'
import { headerLabel, headerStatus, isUrgent } from '@/lib/draftNotesWindow'
import type { NoteStatus } from '@/lib/limited'

// The draft notes window's header (0.x `DraftNotesHeader.vue`): ‹ › pin a
// pick, the label with the countdown, the Live chip and the save state. The
// label drags the window; the buttons do not.
const props = defineProps<{
  pick: DraftNotePickView | null
  hasDraft: boolean
  state: string | null
  isLive: boolean
  picksAhead: number
  canPrev: boolean
  canNext: boolean
  secondsLeft: number | null
  status: NoteStatus
}>()

defineEmits<{ prev: []; next: []; live: [] }>()

const label = computed(() =>
  headerLabel({ hasDraft: props.hasDraft, state: props.state, pick: props.pick, secondsLeft: props.secondsLeft }),
)
const urgent = computed(() => isUrgent(props.secondsLeft, props.pick))
const saved = computed(() => headerStatus(props.status))
</script>

<template>
  <div data-tauri-drag-region class="flex items-center gap-1.5 text-xs font-semibold tracking-tight">
    <div v-if="hasDraft" class="flex shrink-0 items-center">
      <button
        type="button"
        class="rounded p-0.5 text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground disabled:pointer-events-none disabled:opacity-25"
        :disabled="!canPrev"
        title="Previous pick (Alt+←)"
        @click="$emit('prev')"
      >
        <ChevronLeft class="size-3.5" />
      </button>
      <button
        type="button"
        class="rounded p-0.5 text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground disabled:pointer-events-none disabled:opacity-25"
        :disabled="!canNext"
        title="Next pick (Alt+→)"
        @click="$emit('next')"
      >
        <ChevronRight class="size-3.5" />
      </button>
    </div>

    <span data-tauri-drag-region class="min-w-0 flex-1 truncate" :class="urgent ? 'text-rose-400' : ''">{{ label }}</span>

    <template v-if="pick">
      <span v-if="isLive" class="size-1.5 shrink-0 rounded-full bg-emerald-400" title="Following the current pick" />
      <button
        v-else
        type="button"
        class="shrink-0 rounded-full border border-white/10 px-1.5 py-px text-[10px] font-medium text-muted-foreground transition-colors hover:border-emerald-400/40 hover:text-emerald-400"
        title="Back to the current pick"
        @click="$emit('live')"
      >
        Live<span v-if="picksAhead > 0" class="tabular-nums"> +{{ picksAhead }}</span>
      </button>
    </template>

    <span class="inline-flex shrink-0 items-center gap-1 font-normal text-muted-foreground">
      <Loader2 v-if="saved.icon === 'spinner'" class="size-3 animate-spin" />
      <Check v-else-if="saved.icon === 'check'" class="size-3 text-emerald-400" />
      <span v-if="saved.text" :class="status === 'error' ? 'text-rose-400' : ''">{{ saved.text }}</span>
    </span>
  </div>
</template>
