<script setup lang="ts">
import { computed } from 'vue'
import type { RecordView } from '@/bindings'
import { recordParts, type RecordTone } from '@/lib/decks'

const props = withDefaults(
  defineProps<{
    record: RecordView
    /** `letters`: "3W - 1L"; `dashes`: the record's own label, "3 - 1". */
    format?: 'letters' | 'dashes'
    size?: 'xs' | 'sm' | 'base'
    muted?: boolean
    showWinrate?: boolean
  }>(),
  { format: 'letters', size: 'xs', muted: true, showWinrate: false },
)

const sizeClass = { xs: 'text-xs', sm: 'text-sm', base: 'text-base' }
const parts = computed(() => recordParts(props.record))

function toneClass(tone: RecordTone): string {
  if (tone === 'win') return props.muted ? 'text-foreground' : ''
  if (tone === 'loss') return 'text-destructive'
  return ''
}
</script>

<template>
  <span class="tabular-nums" :class="[sizeClass[size], muted ? 'text-muted-foreground' : '']">
    <template v-if="format === 'letters'">
      <template v-for="(part, index) in parts" :key="part.tone">
        <span v-if="index > 0" class="mx-0.5">-</span>
        <span :class="toneClass(part.tone)">{{ part.text }}</span>
      </template>
    </template>
    <template v-else>{{ record.label }}</template>
    <template v-if="showWinrate"> · {{ record.winRate }}%</template>
  </span>
</template>
