<script setup lang="ts">
import { computed, type CSSProperties } from 'vue'
import type { DeckScreenshotView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import {
  CARD_WIDTH,
  STACK_OFFSET,
  cmcBarHeight,
  cmcMax,
  deckFooterText,
  groupIntoStacks,
  groupSideboardStacks,
} from '@/lib/screenshots'

/**
 * The Decklist page's "Share Deck" image (0.x `DeckScreenshot`): name,
 * colours, format, type counts, mana curve, win rate and record, the card
 * stacks and the sideboard. Inline styles and `data:` images only, so the
 * capture does not depend on the page.
 */
const props = defineProps<{ data: DeckScreenshotView }>()

const colors = {
  bg: '#111111',
  text: '#ffffff',
  muted: '#9ca3af',
  border: '#333333',
  bar: '#3b82f6',
  winRate: '#22c55e',
}

const nonLandStacks = computed(() => groupIntoStacks(props.data.nonLandCards))
const landStacks = computed(() => groupIntoStacks(props.data.landCards))
// A commander format's sideboard rows are its commanders: their block comes first.
const commanderFirst = computed(() => props.data.commanderFormat)
const sideboardTitle = computed(() => (commanderFirst.value ? 'Commander' : 'Sideboard'))
const sideboardStacks = computed(() => groupSideboardStacks(props.data.sideboardCards))
const curveMax = computed(() => cmcMax(props.data.cmcDistribution))
const footer = computed(() => deckFooterText(props.data.nonLandCards, props.data.landCards, props.data.sideboardCards))

function imageStyle(index: number): CSSProperties {
  return {
    position: 'absolute',
    width: `${CARD_WIDTH}px`,
    borderRadius: '5px',
    top: `${index * STACK_OFFSET}px`,
    zIndex: index + 1,
  }
}

function stackStyle(height: number): CSSProperties {
  return { position: 'relative', width: `${CARD_WIDTH}px`, height: `${height}px` }
}
</script>

<template>
  <div
    :style="{
      width: '1200px',
      backgroundColor: colors.bg,
      color: colors.text,
      fontFamily: 'system-ui, -apple-system, sans-serif',
      padding: '24px',
      boxSizing: 'border-box',
    }"
  >
    <div
      :style="{
        display: 'flex',
        alignItems: 'center',
        gap: '16px',
        marginBottom: '16px',
        paddingBottom: '12px',
        borderBottom: `1px solid ${colors.border}`,
      }"
    >
      <img
        v-if="data.coverArtBase64"
        :src="data.coverArtBase64"
        alt=""
        :style="{
          width: '64px',
          height: '64px',
          borderRadius: '8px',
          objectFit: 'cover',
          flexShrink: '0',
          border: `2px solid ${colors.border}`,
        }"
      />

      <div :style="{ flexShrink: '0' }">
        <div :style="{ display: 'flex', alignItems: 'center', gap: '10px' }">
          <span :style="{ fontSize: '22px', fontWeight: '700' }">{{ data.name }}</span>
          <ManaColors v-if="data.colors.length" :colors="data.colors" size="lg" />
        </div>
        <div :style="{ fontSize: '13px', color: colors.muted, marginTop: '2px' }">{{ data.format }}</div>
      </div>

      <div :style="{ flex: '1', display: 'flex', alignItems: 'center', justifyContent: 'center', gap: '16px' }">
        <div :style="{ display: 'flex', gap: '10px', fontSize: '11px', color: colors.muted, flexWrap: 'wrap' }">
          <span v-for="entry in data.typeDistribution" :key="entry.type">
            {{ entry.type }}
            <strong :style="{ color: colors.text }">{{ entry.count }}</strong>
          </span>
        </div>
        <div :style="{ display: 'flex', alignItems: 'flex-end', gap: '3px', height: '50px' }">
          <div
            v-for="bucket in data.cmcDistribution"
            :key="bucket.cmc"
            :style="{ display: 'flex', flexDirection: 'column', alignItems: 'center', gap: '2px' }"
          >
            <span :style="{ fontSize: '8px', color: colors.muted }">{{ bucket.count || '' }}</span>
            <div
              :style="{
                width: '16px',
                backgroundColor: colors.bar,
                borderRadius: '2px 2px 0 0',
                height: `${cmcBarHeight(bucket.count, curveMax)}px`,
              }"
            />
            <span :style="{ fontSize: '9px', color: colors.muted }">{{ bucket.cmc }}</span>
          </div>
        </div>
      </div>

      <div :style="{ display: 'flex', alignItems: 'center', gap: '16px' }">
        <div :style="{ textAlign: 'center' }">
          <div :style="{ fontSize: '20px', fontWeight: '700', color: colors.winRate }">{{ data.matchRecord.winRate }}%</div>
          <div :style="{ fontSize: '11px', color: colors.muted }">Win Rate</div>
        </div>
        <div :style="{ textAlign: 'center' }">
          <div :style="{ fontSize: '16px', fontWeight: '600' }">{{ data.matchRecord.label }}</div>
          <div :style="{ fontSize: '11px', color: colors.muted }">Record</div>
        </div>
      </div>
    </div>

    <div :style="{ display: 'flex', gap: '16px' }">
      <div :style="{ flex: '1' }">
        <div :style="{ display: 'flex', flexWrap: 'wrap', gap: '6px' }">
          <div v-for="(stack, i) in nonLandStacks" :key="`nl-${i}`" :style="stackStyle(stack.height)">
            <img
              v-for="(item, j) in stack.items"
              :key="`nl-${i}-${j}`"
              :src="item.imageBase64 ?? undefined"
              :alt="item.name"
              :style="imageStyle(j)"
            />
          </div>
        </div>

        <div :style="{ borderTop: `1px solid ${colors.border}`, margin: '10px 0' }" />

        <div :style="{ display: 'flex', flexWrap: 'wrap', gap: '6px' }">
          <div v-for="(stack, i) in landStacks" :key="`land-${i}`" :style="stackStyle(stack.height)">
            <img
              v-for="(item, j) in stack.items"
              :key="`land-${i}-${j}`"
              :src="item.imageBase64 ?? undefined"
              :alt="item.name"
              :style="imageStyle(j)"
            />
          </div>
        </div>
      </div>

      <div
        v-if="sideboardStacks.length"
        :data-section="commanderFirst ? 'commander' : 'sideboard'"
        :style="{
          flexShrink: '0',
          order: commanderFirst ? -1 : 0,
          ...(commanderFirst
            ? { borderRight: `1px solid ${colors.border}`, paddingRight: '16px' }
            : { borderLeft: `1px solid ${colors.border}`, paddingLeft: '16px' }),
        }"
      >
        <div
          :style="{
            fontSize: '11px',
            color: colors.muted,
            textTransform: 'uppercase',
            letterSpacing: '0.5px',
            marginBottom: '8px',
            fontWeight: '600',
          }"
        >
          {{ sideboardTitle }}
        </div>
        <div :style="{ display: 'flex', flexWrap: 'wrap', gap: '6px', maxWidth: `${CARD_WIDTH * 3 + 12}px` }">
          <div v-for="(stack, i) in sideboardStacks" :key="`sb-${i}`" :style="stackStyle(stack.height)">
            <img
              v-for="(item, j) in stack.items"
              :key="`sb-${i}-${j}`"
              :src="item.imageBase64 ?? undefined"
              :alt="item.name"
              :style="imageStyle(j)"
            />
          </div>
        </div>
      </div>
    </div>

    <div
      :style="{
        display: 'flex',
        justifyContent: 'space-between',
        alignItems: 'center',
        marginTop: '16px',
        paddingTop: '10px',
        borderTop: `1px solid ${colors.border}`,
      }"
    >
      <span :style="{ fontSize: '11px', color: '#555555' }">mymtgo.com</span>
      <span :style="{ fontSize: '10px', color: '#444444' }">{{ footer }}</span>
    </div>
  </div>
</template>
