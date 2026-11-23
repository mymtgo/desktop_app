<script setup lang="ts">
import { useIntervalFn } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import type { LeagueOverlayState } from '@/bindings'
import { displayOf } from '@/lib/leagueOverlayCard'
import { cardSize } from '@/lib/overlaySettings'
import ClassicLeagueCard from './ClassicLeagueCard.vue'
import EventStrip from './EventStrip.vue'
import OverlayLabelInput from './OverlayLabelInput.vue'
import VersusLeagueCard from './VersusLeagueCard.vue'

/**
 * The event card in its theme (league overlay themes spec 9), at its fixed
 * size, with the other event strip below it when `state.other` is present
 * (event overlay spec 8): the card's box never changes, the strip only adds
 * height. Themed cards get the label input through their `title` slot, so
 * the hosted overlay's copies (no Tauri there) render the plain title
 * instead. With Archetype off there is no input, so the card shows its
 * "YOU"; `readonly` shows the title as plain text in both themes. Relative
 * times re-render every 30 s; `now` pins them (tests).
 */
const props = defineProps<{
  state: LeagueOverlayState
  now?: number
  /** The deck title as plain text, not the label input (previews). */
  readonly?: boolean
}>()

const ticking = ref(Date.now())
// Ticks only while no `now` prop pins the time.
const { pause, resume } = useIntervalFn(() => {
  ticking.value = Date.now()
}, 30_000)
watch(
  () => props.now === undefined,
  (unpinned) => (unpinned ? resume() : pause()),
  { immediate: true },
)
const now = computed(() => props.now ?? ticking.value)
const box = computed(() => cardSize(props.state.theme, props.state.size))
</script>

<template>
  <div class="flex flex-col" :style="{ width: `${box.width}px` }">
    <div class="shrink-0" :style="{ height: `${box.height}px` }">
      <VersusLeagueCard v-if="state.theme === 'versus'" :state="state" :now="now">
        <template v-if="state.size === 'full' && displayOf(state).title && !readonly" #title>
          <OverlayLabelInput :state="state" class="h-6 w-full font-[inherit] [font-size:inherit] [letter-spacing:inherit] leading-[inherit] uppercase" />
        </template>
      </VersusLeagueCard>
      <ClassicLeagueCard v-else :state="state" :now="now" :readonly="readonly" />
    </div>
    <Transition name="strip">
      <EventStrip v-if="state.other" :other="state.other" :theme="state.theme" :size="state.size" :now="now" />
    </Transition>
  </div>
</template>

<style scoped>
.strip-enter-active,
.strip-leave-active { transition: opacity 150ms ease; }
.strip-enter-from,
.strip-leave-to { opacity: 0; }
@media (prefers-reduced-motion: reduce) {
  .strip-enter-active,
  .strip-leave-active { transition: none; }
}
</style>
