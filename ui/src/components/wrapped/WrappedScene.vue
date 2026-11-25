<script setup lang="ts">
import { computed } from 'vue'
import type { PlayPeriod } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import { PERIOD_ART } from '@/lib/wrapped/copy'

/**
 * The "When you play" card's backdrop: a basic land's art for the player's
 * part of the day (Wrapped spec), with its artist credited. It fills its
 * parent and sits behind the card's words.
 */
const props = defineProps<{ period: PlayPeriod }>()

const scene = computed(() => PERIOD_ART[props.period])
</script>

<template>
  <div class="wrapped-scene pointer-events-none absolute inset-0" :data-period="period" aria-hidden="true">
    <CardImage
      :src="scene.art.remote"
      :local="scene.art.local"
      :custom="scene.art.custom"
      alt=""
      class="absolute inset-0 size-full object-cover"
    />
  </div>
  <p class="wrapped-credit absolute right-3 bottom-2 z-10 text-[10px] text-white/70 [text-shadow:_0_1px_3px_rgb(0_0_0_/_90%)]">
    {{ scene.credit }}
  </p>
</template>
