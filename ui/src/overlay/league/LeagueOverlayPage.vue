<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { commands, events, type LeagueOverlayState } from '@/bindings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import LeagueOverlayCard from '@/components/overlay/LeagueOverlayCard.vue'
import { unwrap } from '@/lib/commands'

/**
 * The league overlay window (overlay spec 9; 0.x `leagues/Overlay.vue`). The
 * first card comes from `league_overlay`, then `league-overlay-changed`
 * pushes replace it (nothing polls). The whole window drags it; the label
 * input inside is not a drag region.
 */
const state = ref<LeagueOverlayState | null>(null)

let pushes = 0
const listening = useTauriEvents(() => [
  events.leagueOverlayChanged.listen((event) => {
    pushes += 1
    state.value = event.payload.state
  }),
])

onMounted(async () => {
  if (!(await listening.ready)) return
  const before = pushes
  const first = unwrap(await commands.leagueOverlay())
  // A push that arrived meanwhile is newer than what the read saw.
  if (pushes === before) state.value = first
})
</script>

<template>
  <div data-tauri-drag-region="deep" class="h-screen w-screen cursor-move">
    <LeagueOverlayCard v-if="state" :state="state" />
  </div>
</template>
