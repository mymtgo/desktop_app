<script setup lang="ts">
// The running in and out tally of a plan and whether it leaves a legal board
// (0.x `SideboardGuideBalance`). The board has to come back to the size it
// started at, so a plan is finished when everything brought in is paid for.
import { ArrowLeft, ArrowRight, Check } from 'lucide-vue-next'
import { computed } from 'vue'
import { balance } from '@/lib/guides'

const props = defineProps<{
  totalIn: number
  totalOut: number
  /** Copies in the deck's sideboard, which the plan has to return to. */
  sideboardSize: number
}>()

const chip = computed(() => balance(props.totalIn, props.totalOut, props.sideboardSize))
</script>

<template>
  <div
    class="flex items-center gap-2.5 rounded-full border px-3 py-1.5 shadow-sm backdrop-blur"
    :class="chip.hint ? 'border-amber-500/40 bg-amber-500/10' : 'border-border bg-background/95'"
  >
    <span class="flex items-center gap-1.5">
      <span class="text-sm leading-none font-semibold tabular-nums">{{ totalIn }}</span>
      <span class="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">in</span>
      <ArrowRight class="size-3 shrink-0 text-muted-foreground/50" />
    </span>
    <span class="flex items-center gap-1.5">
      <ArrowLeft class="size-3 shrink-0 text-muted-foreground/50" />
      <span class="text-sm leading-none font-semibold tabular-nums">{{ totalOut }}</span>
      <span class="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">out</span>
    </span>

    <span class="h-3.5 w-px shrink-0" :class="chip.hint ? 'bg-amber-500/30' : 'bg-border'"></span>

    <span
      class="flex items-center gap-1.5 text-[10px] font-semibold tracking-wider uppercase tabular-nums"
      :class="chip.hint ? 'text-amber-300' : 'text-muted-foreground'"
    >
      board {{ chip.resulting }} / {{ sideboardSize }}
      <template v-if="chip.hint">· {{ chip.hint }}</template>
      <Check v-else class="size-3" aria-label="Balanced" />
    </span>
  </div>
</template>
