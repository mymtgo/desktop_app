<script setup lang="ts">
import type { LimitedIndexQuery, PickSetOptionView } from '@/bindings'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { LIMITED_TIMEFRAMES } from '@/lib/limited'

/** Set, kind and timeframe (0.x `LimitedFilters`). */
const props = defineProps<{ filters: LimitedIndexQuery & { timeframe: string }; sets: PickSetOptionView[] }>()

const emit = defineEmits<{ change: [next: LimitedIndexQuery & { timeframe: string }] }>()

const ALL = '__all__'

function update(patch: Partial<LimitedIndexQuery>) {
  emit('change', { ...props.filters, ...patch, timeframe: patch.timeframe ?? props.filters.timeframe })
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2">
    <Select
      :model-value="filters.set ?? ALL"
      @update:model-value="(value) => update({ set: value === ALL ? null : String(value) })"
    >
      <SelectTrigger class="h-8 w-36" aria-label="Filter by set">
        <SelectValue placeholder="All sets" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem :value="ALL">All sets</SelectItem>
        <SelectItem v-for="set in sets" :key="set.code" :value="set.code">{{ set.name }}</SelectItem>
      </SelectContent>
    </Select>

    <Select
      :model-value="filters.kind ?? ALL"
      @update:model-value="(value) => update({ kind: value === ALL ? null : String(value) })"
    >
      <SelectTrigger class="h-8 w-40" aria-label="Filter by event kind">
        <SelectValue placeholder="Draft &amp; Sealed" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem :value="ALL">Draft &amp; Sealed</SelectItem>
        <SelectItem value="draft">Draft</SelectItem>
        <SelectItem value="sealed">Sealed</SelectItem>
      </SelectContent>
    </Select>

    <SegmentedControl
      class="ml-auto"
      :model-value="filters.timeframe"
      :options="LIMITED_TIMEFRAMES"
      @update:model-value="(value) => update({ timeframe: value })"
    />
  </div>
</template>
