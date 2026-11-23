<script setup lang="ts">
import type { ColorSignalView } from '@/bindings'
import ManaPips from '@/components/limited/ManaPips.vue'
import { CardContent } from '@/components/ui/card'
import { signalText, signalWidth } from '@/lib/limited'
import { MANA_COLOR_NAMES } from '@/lib/mana'

/** Per colour: cards wheeled and seen twice or more, as a bar against the top colour (0.x `SignalsPanel`). */
defineProps<{ signals: ColorSignalView[] }>()
</script>

<template>
  <div class="grid grid-cols-5 py-4">
    <CardContent v-for="signal in signals" :key="signal.color" class="flex flex-col gap-1.5">
      <div class="flex items-center gap-2 text-xs">
        <ManaPips :colors="signal.color" size="md" />
        <span class="font-medium">{{ MANA_COLOR_NAMES[signal.color] }}</span>
        <span class="ml-auto text-muted-foreground tabular-nums">{{ signalText(signal) }}</span>
      </div>
      <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div class="bevel h-full rounded-full bg-primary" :style="{ width: signalWidth(signal.share) }" />
      </div>
    </CardContent>
  </div>
</template>
