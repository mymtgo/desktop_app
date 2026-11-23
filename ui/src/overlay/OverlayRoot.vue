<script setup lang="ts">
import { computed } from 'vue'
import { hasTitleStrip, isTransparent, type OverlayView, viewTitle } from './views'

// One overlay window's page. The window has no title bar: the strip at the
// top drags it (`data-tauri-drag-region`; controls inside a strip are not
// drag regions). Plan U tasks 7 to 9 mount the game, draft notes and league
// pages here; the league card fills its window with no strip.
const props = defineProps<{ view: OverlayView | null }>()

const title = computed(() => (props.view ? viewTitle(props.view) : 'Overlay'))
const strip = computed(() => hasTitleStrip(props.view))
// The league window is transparent: only its rounded card shows.
const seeThrough = computed(() => isTransparent(props.view))
</script>

<template>
  <div
    class="flex h-screen flex-col overflow-hidden text-foreground select-none"
    :class="seeThrough ? 'bg-transparent' : 'bg-background'"
  >
    <header v-if="strip" data-tauri-drag-region class="flex h-7 shrink-0 cursor-move items-center px-3 text-xs text-muted-foreground">
      <span data-tauri-drag-region class="truncate">{{ title }}</span>
    </header>
    <main class="min-h-0 flex-1 overflow-auto">
      <slot />
    </main>
  </div>
</template>
