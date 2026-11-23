<script setup lang="ts">
import { computed } from 'vue'
import type { LeagueTheme, OverlaySize } from '@/bindings'
import type { OtherEvent } from '@/bindings'
import { stripLive, stripSegments } from '@/lib/eventStrip'

/**
 * The other event strip (event overlay spec 8): a tab attached under the
 * card, the card's width less its inset, 16 px tall (20 on Versus Full).
 * One line, ellipsis when long; the time and records bold.
 */
const props = defineProps<{ other: OtherEvent; theme: LeagueTheme; size: OverlaySize; now: number }>()

const compact = computed(() => props.size === 'compact')
const segments = computed(() => stripSegments(props.other, props.now, compact.value))
const live = computed(() => stripLive(props.other, props.now))
</script>

<template>
  <div
    class="strip"
    :class="[theme === 'versus' ? 'versus' : 'classic', compact ? 'compact' : 'full']"
    data-strip
  >
    <i v-if="theme !== 'versus'" class="dot" :class="{ wait: !live }" />
    <span class="text">
      <template v-for="(segment, index) in segments" :key="index">
        <b v-if="segment.bold">{{ segment.text }}</b>
        <template v-else>{{ segment.text }}</template>
      </template>
    </span>
  </div>
</template>

<style scoped>
/* Event overlay spec 8.1 and 8.2: a 16 px tab (20 on Versus Full), inset 10 px under the card. */
.strip { display: flex; align-items: center; margin: 0 10px; overflow: hidden; white-space: nowrap; text-transform: uppercase; box-sizing: border-box; }
.text { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
b { color: #fff; font-weight: 700; }
.dot { flex-shrink: 0; width: 6px; height: 6px; border-radius: 50%; background: #7dd3fc; box-shadow: 0 0 6px rgb(125 211 252 / 0.7); }
.dot.wait { background: rgb(255 255 255 / 0.4); box-shadow: none; }

.classic { height: 16px; padding: 2px 8px 2px 10px; gap: 6px; background: rgb(12 16 20 / 0.9); font-family: var(--font-card); font-size: 10px; line-height: 12px; font-weight: 700; letter-spacing: 0.1em; color: rgb(255 255 255 / 0.85); text-shadow: 0 1px 2px rgb(0 0 0 / 0.8); }
.classic.full { border-radius: 0 0 10px 10px; }
.classic.compact { border-radius: 0 0 8px 8px; padding-left: 6px; }

.versus { gap: 8px; font-family: 'Teko', sans-serif; background: linear-gradient(180deg, rgb(8 10 16 / 0.92), rgb(8 10 16 / 0.78)); border-bottom: 2px solid #22d3ee; border-radius: 0 0 6px 6px; color: #aab3c5; letter-spacing: 2px; }
.versus.full { height: 20px; padding: 2px 14px 0; font-size: 15px; line-height: 16px; }
.versus.compact { height: 16px; margin: 0 8px; padding: 2px 4px 0; font-size: 13px; line-height: 12px; }
</style>
