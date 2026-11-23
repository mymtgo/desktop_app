<script setup lang="ts">
import { computed } from 'vue'
import type { PreviewSample } from '@/components/overlay/overlaySamples'
import SegmentedControl from '@/components/SegmentedControl.vue'
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import type { EventKind } from '@/bindings'
import { firstPreview, previewGroups } from '@/lib/overlaySettings'

/**
 * The event card preview's pickers (spec 4.2, 4.3): League | Challenge and
 * the state, grouped Playing, Between matches, Finished. The page and the
 * slide-out each show a copy bound to the same preview state.
 */
withDefaults(defineProps<{ /** The state picker takes the rest of the row (the slide-out). */ fill?: boolean }>(), {
  fill: false,
})

const kind = defineModel<EventKind>('kind', { required: true })
const sample = defineModel<PreviewSample>('sample', { required: true })

const KIND_OPTIONS: Array<{ value: EventKind; label: string }> = [
  { value: 'league', label: 'League' },
  { value: 'tournament', label: 'Challenge' },
]

const groups = computed(() => previewGroups(kind.value).filter((group) => group.options.length > 0))

function chooseKind(next: EventKind): void {
  if (next === kind.value) return
  kind.value = next
  sample.value = firstPreview(next)
}
</script>

<template>
  <div class="flex items-center gap-2" :class="fill ? 'w-full' : 'flex-wrap'">
    <SegmentedControl
      :model-value="kind"
      :options="KIND_OPTIONS"
      class="shrink-0"
      aria-label="Preview event kind"
      @update:model-value="(value: string) => chooseKind(value as EventKind)"
    />
    <Select v-model="sample">
      <SelectTrigger :class="fill ? 'min-w-0 flex-1' : 'w-56'" aria-label="Preview state">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        <template v-for="(group, index) in groups" :key="group.group">
          <SelectSeparator v-if="index > 0" />
          <SelectGroup>
            <SelectLabel>{{ group.label }}</SelectLabel>
            <SelectItem v-for="option in group.options" :key="option.value" :value="option.value">
              {{ option.label }}
            </SelectItem>
          </SelectGroup>
        </template>
      </SelectContent>
    </Select>
  </div>
</template>
