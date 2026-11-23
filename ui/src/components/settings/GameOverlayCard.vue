<script setup lang="ts">
import { computed } from 'vue'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import type { Settings } from '@/bindings'

// 0.x GameOverlayCard: the master switch and the four section switches
// (disabled while the master is off). A section never touches the master.
// Unavailable on native Wayland (overlay spec 5.3).
const props = defineProps<{
  settings: Settings
  note: string | null
  disabled: boolean
}>()

const emit = defineEmits<{ change: [change: Partial<Settings>] }>()

type SectionKey = 'overlayShowOpponent' | 'overlayShowDrawOdds' | 'overlayShowReveals' | 'overlayShowSideboard'

const sections: Array<{ key: SectionKey; label: string }> = [
  { key: 'overlayShowOpponent', label: 'Show opponent scout' },
  { key: 'overlayShowDrawOdds', label: 'Show draw odds' },
  { key: 'overlayShowReveals', label: 'Show revealed cards' },
  { key: 'overlayShowSideboard', label: 'Show sideboard guide' },
]

const enabled = computed(() => props.settings.gameOverlay)
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Game overlay</h2>
      <p class="text-xs text-muted-foreground">
        A floating panel during matches with your opponent's archetype, live draw odds, and your sideboard guide.
      </p>
    </div>
    <p v-if="note" class="text-sm text-warning">{{ note }}</p>
    <div class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <Label>Show game overlay</Label>
        <p class="text-sm text-muted-foreground">Opens when a match starts and closes when it ends.</p>
      </div>
      <Switch
        :model-value="enabled"
        :disabled="disabled"
        @update:model-value="(on: boolean) => emit('change', { gameOverlay: on })"
      />
    </div>

    <div class="flex flex-col gap-3 pl-6">
      <div v-for="section in sections" :key="section.key" class="flex items-center justify-between">
        <Label :class="enabled ? '' : 'text-muted-foreground'">{{ section.label }}</Label>
        <Switch
          :model-value="settings[section.key]"
          :disabled="!enabled || disabled"
          @update:model-value="(on: boolean) => emit('change', { [section.key]: on })"
        />
      </div>
    </div>
  </section>
</template>
