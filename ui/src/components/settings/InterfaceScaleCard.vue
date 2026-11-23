<script setup lang="ts">
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { useSettings } from '@/lib/queries'

// Must match INTERFACE_SCALE_OPTIONS in app/src/settings.rs.
const SCALES = [80, 90, 100, 110, 125, 150]

const { data: settings } = useSettings()
const { saving, save } = useSaveSettings()

function setScale(value: unknown) {
  if (settings.value) void save({ ...settings.value, interfaceScale: Number(value) })
}
</script>

<template>
  <section class="space-y-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Appearance</h2>
      <p class="text-xs text-muted-foreground">Make the app window larger or smaller, for example on a 4K monitor.</p>
    </div>
    <div v-if="settings" class="flex items-center justify-between gap-6">
      <div class="space-y-1">
        <p class="text-sm font-medium">Interface scale</p>
        <p class="text-xs text-muted-foreground">Overlays keep their own size for OBS.</p>
      </div>
      <Select :model-value="String(settings.interfaceScale)" :disabled="saving" @update:model-value="setScale">
        <SelectTrigger size="sm" class="w-24 text-xs">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="scale in SCALES" :key="scale" :value="String(scale)" class="text-xs">{{ scale }}%</SelectItem>
        </SelectContent>
      </Select>
    </div>
  </section>
</template>
