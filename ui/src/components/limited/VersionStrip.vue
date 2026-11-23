<script setup lang="ts">
import { computed } from 'vue'
import type { RegisteredVersionView } from '@/bindings'
import ManaPips from '@/components/limited/ManaPips.vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { versionLine, versionOptions } from '@/lib/limitedPages'

/**
 * Which registered build the whole page is showing (0.x `VersionStrip`).
 * Sits above the zones because it drives every card below it.
 */
const props = defineProps<{ versions: RegisteredVersionView[]; selected: number | null }>()
const emit = defineEmits<{ select: [index: number] }>()

const options = computed(() => versionOptions(props.versions))
const active = computed(() => props.versions.find((version) => version.index === props.selected) ?? null)
const line = computed(() => (active.value ? versionLine(active.value) : null))
</script>

<template>
  <div class="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-lg border border-black/60 bg-card px-4 py-2 text-xs">
    <SegmentedControl
      v-if="versions.length > 1"
      :model-value="String(selected ?? '')"
      :options="options"
      @update:model-value="(value) => emit('select', Number(value))"
    />
    <template v-if="active && line">
      <span class="font-medium">{{ line.title }}</span>
      <span class="text-muted-foreground">{{ line.matches }}</span>
      <span class="text-muted-foreground">{{ line.time }}</span>
      <span class="text-muted-foreground tabular-nums">{{ line.counts }}</span>
      <ManaPips :colors="active.colors" size="md" />
    </template>
    <span v-else class="text-muted-foreground">No registered deck yet.</span>
  </div>
</template>
