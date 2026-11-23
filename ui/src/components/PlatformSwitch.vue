<script setup lang="ts">
import { computed } from 'vue'
import type { PlatformContextView, PlatformView } from '@/bindings'
import arenaLogo from '@/assets/platforms/arena.svg'
import endstepLogo from '@/assets/platforms/endstep.svg'
import mtgoLogo from '@/assets/platforms/mtgo.svg'

const props = defineProps<{
  context: PlatformContextView
  /** The platforms with a match in progress. */
  live: PlatformView[]
}>()

const emit = defineEmits<{
  switch: [platform: PlatformView]
}>()

const allOptions: { platform: PlatformView; label: string; logo: string }[] = [
  { platform: 'mtgo', label: 'MTGO', logo: mtgoLogo },
  { platform: 'arena', label: 'MTG Arena', logo: arenaLogo },
  { platform: 'endstep', label: 'Endstep', logo: endstepLogo },
]

/** Only the platforms that are switched on (and have data) get a logo. */
const options = computed(() => allOptions.filter((option) => props.context[option.platform]))

function choose(platform: PlatformView) {
  if (platform !== props.context.current) emit('switch', platform)
}
</script>

<template>
  <div v-if="context.switchable" class="flex shrink-0 items-center gap-1" data-platform-switch>
    <button
      v-for="option in options"
      :key="option.platform"
      type="button"
      :aria-pressed="context.current === option.platform"
      :aria-label="option.label"
      class="relative cursor-pointer rounded-md border border-black/60 px-2 py-1 transition-all"
      :class="
        context.current === option.platform
          ? 'border-black shadow-inner shadow-black'
          : 'bevel opacity-60 hover:opacity-100'
      "
      @click="choose(option.platform)"
    >
      <img :src="option.logo" :alt="option.label" class="h-5 w-auto brightness-0 invert" />
      <span
        v-if="live.includes(option.platform) && context.current !== option.platform"
        data-live-dot
        class="absolute -top-0.5 -right-0.5 size-1.5 rounded-full bg-warning"
      />
    </button>
  </div>
</template>
