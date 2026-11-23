<script setup lang="ts">
import { ChevronDown, ChevronUp } from 'lucide-vue-next'
import { computed } from 'vue'
import type { Sections } from '@/bindings'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { NO_SECTIONS, expandsOnTabPick, overlayTabs, toggleLabel } from '@/lib/overlay'

/**
 * The overlay's tabs (0.x `OverlayTabs`): "Draw odds", "Revealed",
 * "Sideboarding" for the sections on. One section shows its name as a bar
 * with the collapse toggle. Collapsed, only the bar shows; picking a tab
 * while collapsed expands it.
 */
const props = defineProps<{
  modelValue: string
  sections: Sections
  collapsed: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
  'update:collapsed': [value: boolean]
}>()

const tabs = computed(() => overlayTabs(props.sections))
const label = computed(() => toggleLabel(props.collapsed))

function picked(): void {
  if (expandsOnTabPick(props.collapsed)) emit('update:collapsed', false)
}
</script>

<template>
  <div v-if="tabs.length <= 1" class="flex min-h-0 flex-col" :class="!collapsed && 'flex-1'">
    <div v-if="tabs[0]" data-overlay-bar data-tauri-drag-region class="flex shrink-0 items-center gap-2 border-b border-border py-1 pr-1 pl-3">
      <span data-tauri-drag-region class="flex-1 text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">{{ tabs[0].label }}</span>
      <button
        type="button"
        class="flex size-7 cursor-pointer items-center justify-center rounded text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        :aria-label="label"
        :title="label"
        :aria-expanded="!collapsed"
        @click="emit('update:collapsed', !collapsed)"
      >
        <ChevronDown v-if="collapsed" class="size-4" />
        <ChevronUp v-else class="size-4" />
      </button>
    </div>
    <div v-if="!collapsed" class="min-h-0 flex-1 overflow-y-auto">
      <slot v-if="tabs[0]" :name="tabs[0].value" />
      <p v-else class="p-3 text-center text-xs text-muted-foreground">{{ NO_SECTIONS }}</p>
    </div>
  </div>

  <Tabs
    v-else
    :model-value="props.modelValue"
    class="flex min-h-0 flex-col gap-0"
    :class="!collapsed && 'flex-1'"
    @update:model-value="emit('update:modelValue', String($event))"
  >
    <div data-overlay-bar class="flex shrink-0 items-center border-b border-border bg-black/50 pr-1">
      <TabsList class="min-w-0 flex-1 rounded-none border-0 bg-transparent">
        <TabsTrigger
          v-for="tab in tabs"
          :key="tab.value"
          :value="tab.value"
          :class="['flex-1 text-xs', collapsed && 'data-[state=active]:bg-transparent data-[state=active]:shadow-none']"
          @click="picked"
        >
          {{ tab.label }}
        </TabsTrigger>
      </TabsList>
      <button
        type="button"
        class="flex size-7 shrink-0 cursor-pointer items-center justify-center rounded text-muted-foreground transition-colors hover:bg-white/5 hover:text-foreground"
        :aria-label="label"
        :title="label"
        :aria-expanded="!collapsed"
        @click="emit('update:collapsed', !collapsed)"
      >
        <ChevronDown v-if="collapsed" class="size-4" />
        <ChevronUp v-else class="size-4" />
      </button>
    </div>

    <template v-if="!collapsed">
      <TabsContent v-for="tab in tabs" :key="tab.value" :value="tab.value" class="min-h-0 flex-1 overflow-y-auto">
        <slot :name="tab.value" />
      </TabsContent>
    </template>
  </Tabs>
</template>
