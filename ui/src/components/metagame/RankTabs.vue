<script setup lang="ts">
// The Arena metagame's rank tabs: All, then one icon button per band. A band
// without matches is disabled and dimmed. `null` is All.
import type { MetagameBandView } from '@/bindings'
import { rankIconFile } from '@/lib/metagame'
import { rankIconUrlByName } from '@/lib/rankIcons'

defineProps<{ bands: MetagameBandView[] }>()
const model = defineModel<string | null>({ required: true })

function iconUrl(key: string): string | undefined {
  return rankIconUrlByName(rankIconFile(key))
}

// A disabled tab is never shown active, even if the route names its band.
function isActive(band: MetagameBandView): boolean {
  return band.matches > 0 && model.value === band.key
}

const base = 'relative cursor-pointer rounded border border-black transition-all disabled:cursor-not-allowed disabled:opacity-40'
const inactive = 'bevel border bg-background text-muted-foreground hover:text-foreground hover:brightness-125'
</script>

<template>
  <div role="group" aria-label="Rank" class="flex items-center gap-1">
    <button
      type="button"
      :aria-pressed="model === null"
      :class="[base, 'px-4 py-2 text-xs font-medium', model === null ? 'nav-item-active' : inactive]"
      @click="model = null"
    >
      All
    </button>
    <button
      v-for="band in bands"
      :key="band.key"
      type="button"
      :disabled="band.matches === 0"
      :aria-pressed="isActive(band)"
      :aria-label="band.label"
      :title="band.label"
      :class="[base, 'p-1', isActive(band) ? 'nav-item-active' : inactive]"
      @click="model = band.key"
    >
      <img v-if="iconUrl(band.key)" :src="iconUrl(band.key)" :alt="band.label" class="size-8 object-contain" />
      <span v-else class="px-2 text-xs font-medium">{{ band.label }}</span>
    </button>
  </div>
</template>
