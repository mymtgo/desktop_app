<script setup lang="ts">
import { computed } from 'vue'
import type { CardStatView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { colorLetters } from '@/lib/archetypes'
import type { Visibility } from '@/lib/cardStats/columns'
import { cardStatBlocks, contextChips, type Tone } from '@/lib/screenshots'

/**
 * The Card Stats row menu's "Copy screenshot" (0.x `CardStatScreenshot`):
 * the card, its colours and type, the opposing archetype when one is
 * picked, the filter chips, the card image and a block per visible column.
 * The image is a `data:` URL (`card_image_data_url`), so the capture is
 * complete.
 */
const props = defineProps<{
  stat: CardStatView
  visibleColumns: Visibility
  timeframeLabel: string
  archetypeName?: string | null
  archetypeColorIdentity?: string | null
  boardLabel?: string | null
  playDrawLabel?: string | null
  imageDataUrl?: string | null
}>()

const colors = {
  bg: '#111111',
  text: '#ffffff',
  muted: 'rgba(255, 255, 255, 0.5)',
  subtle: 'rgba(255, 255, 255, 0.04)',
  win: '#22c55e',
  loss: '#ef4444',
  border: 'rgba(255, 255, 255, 0.08)',
}

function toneColor(tone: Tone | undefined): string {
  if (tone === 'good') return colors.win
  if (tone === 'bad') return colors.loss
  return colors.text
}

const cardColors = computed(() => colorLetters(props.stat.colorIdentity))
const archetypeColors = computed(() => colorLetters(props.archetypeColorIdentity))
const blocks = computed(() => cardStatBlocks(props.stat, props.visibleColumns))
const chips = computed(() => contextChips(props.boardLabel ?? null, props.playDrawLabel ?? null, props.timeframeLabel))
</script>

<template>
  <div
    :style="{
      width: '640px',
      backgroundColor: colors.bg,
      color: colors.text,
      fontFamily: 'system-ui, -apple-system, sans-serif',
      padding: '20px 22px',
      borderRadius: '12px',
      position: 'relative',
      overflow: 'hidden',
    }"
  >
    <div :style="{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: '16px' }">
      <div :style="{ minWidth: 0, flex: 1 }">
        <div :style="{ display: 'flex', alignItems: 'center', gap: '8px', flexWrap: 'wrap' }">
          <span :style="{ fontSize: '20px', fontWeight: '700', lineHeight: '1.2' }">{{ stat.name }}</span>
          <ManaColors v-if="cardColors.length" :colors="cardColors" size="sm" />
          <span
            v-if="stat.isSideboard"
            :style="{
              fontSize: '9px',
              fontWeight: '600',
              letterSpacing: '0.08em',
              textTransform: 'uppercase',
              padding: '2px 6px',
              borderRadius: '4px',
              backgroundColor: colors.subtle,
              color: colors.muted,
            }"
            >Sideboard</span
          >
        </div>
        <div v-if="stat.type" :style="{ fontSize: '11px', color: colors.muted, marginTop: '2px' }">{{ stat.type }}</div>
      </div>
      <div v-if="archetypeName" :style="{ textAlign: 'right', flexShrink: 0 }">
        <div
          :style="{
            fontSize: '9px',
            fontWeight: '600',
            letterSpacing: '0.08em',
            textTransform: 'uppercase',
            color: colors.muted,
          }"
        >
          Versus
        </div>
        <div :style="{ display: 'flex', alignItems: 'center', justifyContent: 'flex-end', gap: '6px', marginTop: '3px' }">
          <span :style="{ fontSize: '14px', fontWeight: '600', lineHeight: '1.2' }">{{ archetypeName }}</span>
          <ManaColors v-if="archetypeColors.length" :colors="archetypeColors" size="sm" />
        </div>
      </div>
    </div>

    <div :style="{ display: 'flex', flexWrap: 'wrap', gap: '6px', marginTop: '10px' }">
      <span
        v-for="(chip, i) in chips"
        :key="i"
        :style="{
          fontSize: '10px',
          fontWeight: '500',
          letterSpacing: '0.04em',
          textTransform: 'uppercase',
          color: colors.muted,
          backgroundColor: colors.subtle,
          padding: '3px 8px',
          borderRadius: '999px',
        }"
        >{{ chip }}</span
      >
    </div>

    <div :style="{ height: '1px', backgroundColor: colors.border, margin: '14px 0' }" />

    <div :style="{ display: 'flex', gap: '16px', alignItems: 'flex-start' }">
      <div
        :style="{
          width: '170px',
          flexShrink: 0,
          aspectRatio: '488 / 680',
          borderRadius: '10px',
          overflow: 'hidden',
          backgroundColor: colors.subtle,
        }"
      >
        <img
          v-if="imageDataUrl"
          :src="imageDataUrl"
          :alt="stat.name"
          :style="{ width: '100%', height: '100%', display: 'block', objectFit: 'cover' }"
        />
      </div>

      <div :style="{ flex: 1, display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: '6px' }">
        <div
          v-for="(block, i) in blocks"
          :key="i"
          :style="{
            backgroundColor: colors.subtle,
            padding: '8px 10px',
            borderRadius: '6px',
            display: 'flex',
            flexDirection: 'column',
            gap: '2px',
          }"
        >
          <div
            :style="{
              fontSize: '9px',
              fontWeight: '600',
              letterSpacing: '0.06em',
              textTransform: 'uppercase',
              color: colors.muted,
            }"
          >
            {{ block.label }}
          </div>
          <div :style="{ display: 'flex', alignItems: 'baseline', gap: '4px' }">
            <span
              :style="{
                fontSize: '17px',
                fontWeight: '700',
                color: toneColor(block.tone),
                fontVariantNumeric: 'tabular-nums',
                lineHeight: '1.1',
              }"
            >
              {{ block.value }}
            </span>
            <span v-if="block.sub" :style="{ fontSize: '10px', color: colors.muted, fontVariantNumeric: 'tabular-nums' }">
              {{ block.sub }}
            </span>
          </div>
        </div>
      </div>
    </div>

    <div :style="{ marginTop: '14px', textAlign: 'right', fontSize: '10px', color: colors.muted }">mymtgo.com</div>
  </div>
</template>
