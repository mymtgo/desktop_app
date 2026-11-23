<script setup lang="ts">
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import type { Settings } from '@/bindings'

// 0.x DraftNotesCard. Unavailable on native Wayland (overlay spec 5.3): the
// switch is disabled and the stored value kept.
defineProps<{
  enabled: boolean
  note: string | null
  disabled: boolean
}>()

const emit = defineEmits<{ change: [change: Partial<Settings>] }>()
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Draft notes window</h2>
      <p class="text-xs text-muted-foreground">A small always-on-top window shown while you draft.</p>
    </div>
    <p v-if="note" class="text-sm text-warning">{{ note }}</p>
    <div class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <Label>Show draft notes window</Label>
        <p class="text-sm text-muted-foreground">
          Shows the current pick, its timer, and a place to jot why you took the card.
        </p>
      </div>
      <Switch
        :model-value="enabled"
        :disabled="disabled"
        @update:model-value="(on: boolean) => emit('change', { draftNotesWindow: on })"
      />
    </div>
  </section>
</template>
