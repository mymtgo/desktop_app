<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { WrappedBundle, WrappedCard } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import { Card } from '@/components/ui/card'
import WrappedActivity from '@/components/wrapped/WrappedActivity.vue'
import WrappedBannedCard from '@/components/wrapped/WrappedBannedCard.vue'
import WrappedDeckCard from '@/components/wrapped/WrappedDeckCard.vue'
import WrappedLimitedCard from '@/components/wrapped/WrappedLimitedCard.vue'
import WrappedMagicYearCard from '@/components/wrapped/WrappedMagicYearCard.vue'
import WrappedPlayerTypeCard from '@/components/wrapped/WrappedPlayerTypeCard.vue'
import WrappedScene from '@/components/wrapped/WrappedScene.vue'
import WrappedSpotlightCard from '@/components/wrapped/WrappedSpotlightCard.vue'
import WrappedWrapCard from '@/components/wrapped/WrappedWrapCard.vue'
import { backdropArt, cardView, count, hourText } from '@/lib/wrapped/copy'

/**
 * One Wrapped card (Wrapped spec, mockup sections 2 to 4): the app's Card,
 * a small label, a big number or a title, a line or two, figures, and art on
 * the right when the card has it. The numbers card sits over the deck of
 * the year's art, faint enough for the text to read over it. The words
 * come from `cardView`.
 */
const props = defineProps<{ card: WrappedCard; bundle: WrappedBundle }>()
const emit = defineEmits<{ restart: [] }>()

const view = computed(() => cardView(props.card, props.bundle))
// The opening card has the deck of the year's art faintly behind it.
const backdrop = computed(() => (props.card.kind === 'yearInNumbers' ? backdropArt(props.bundle) : null))

// Each source the image tries (custom cover, downloaded file, remote art)
// can fail once; when all have, the card drops to the no-art layout rather
// than showing a broken image or an empty half.
const artErrors = ref(0)
const artSources = computed(() => {
  const art = view.value.art
  if (art === null) return 0
  return [art.custom, art.local, art.remote].filter((source) => source !== null).length
})
watch(
  () => props.card,
  () => {
    artErrors.value = 0
  },
)
const hasArt = computed(() => artSources.value > 0 && artErrors.value < artSources.value)

function onArtError() {
  artErrors.value += 1
}

// White at the strength of each hour's share of the busiest hour, so the
// strip sits quietly on any scene.
function hourShade(value: number, max: number): string {
  if (max === 0 || value === 0) return 'bg-white/10'
  const share = value / max
  if (share > 0.8) return 'bg-white/85'
  if (share > 0.55) return 'bg-white/60'
  if (share > 0.3) return 'bg-white/35'
  return 'bg-white/20'
}

function plural(matches: number): string {
  return `${count(matches)} ${matches === 1 ? 'match' : 'matches'}`
}

const maxHour = computed(() => (view.value.visual?.kind === 'hours' ? Math.max(0, ...view.value.visual.byHour) : 0))
</script>

<template>
  <WrappedMagicYearCard v-if="card.kind === 'magicYear'" :view="view" />
  <WrappedBannedCard v-else-if="card.kind === 'banned'" :view="view" />
  <WrappedLimitedCard v-else-if="card.kind === 'limited'" :view="view" />
  <WrappedPlayerTypeCard v-else-if="card.kind === 'playerType'" :view="view" :year="bundle.year" />
  <WrappedWrapCard v-else-if="card.kind === 'wrap'" :view="view" :year="bundle.year" @restart="emit('restart')" />
  <WrappedDeckCard v-else-if="view.deck" :view="view" />
  <WrappedSpotlightCard v-else-if="view.layout === 'spotlight'" :view="view" />
  <Card v-else class="relative grid h-full gap-0 overflow-hidden py-0" :class="hasArt ? 'grid-cols-[1.1fr_1fr]' : 'grid-cols-1'">
    <div v-if="backdrop" class="wrapped-backdrop pointer-events-none absolute inset-0 overflow-hidden" aria-hidden="true">
      <CardImage
        :src="backdrop.remote"
        :local="backdrop.local"
        :custom="backdrop.custom"
        alt=""
        class="absolute inset-0 size-full scale-105 object-cover opacity-30 blur-[3px] saturate-50"
      />
      <!-- Fades the art out towards the foot so the grid and figures sit on calm ground. -->
      <div class="absolute inset-0 bg-linear-to-b from-card/30 via-card/75 to-card" />
    </div>
    <template v-if="view.scene">
      <WrappedScene :period="view.scene" />
      <div class="wrapped-scene-shade pointer-events-none absolute inset-0" aria-hidden="true" />
    </template>
    <div
      class="relative flex min-w-0 flex-col p-8"
      :class="view.scene ? 'max-w-[62%]' : ''"
    >
      <p class="text-xs font-medium uppercase tracking-widest text-muted-foreground">{{ view.label }}</p>

      <p v-if="view.big" class="mt-3 text-6xl font-extrabold tracking-tight tabular-nums">{{ view.big }}</p>
      <h2 v-if="view.title" class="wrapped-title mt-3 line-clamp-2 text-3xl font-extrabold leading-tight">
        {{ view.title }}
      </h2>

      <p v-if="view.lead" class="wrapped-lead mt-3 max-w-prose text-lg leading-snug text-balance text-foreground/90">{{ view.lead }}</p>

      <p v-for="line in view.lines" :key="line" class="mt-2 max-w-prose text-sm leading-relaxed text-pretty text-foreground/85">
        {{ line }}
      </p>

      <div v-if="view.visual?.kind === 'mentions'" class="mt-auto">
        <p class="mb-2 text-sm font-medium text-muted-foreground">Honourable mentions</p>
        <ul class="flex flex-col gap-2">
          <li
            v-for="row in view.visual.rows"
            :key="row.label"
            class="wrapped-mention wrapped-surface flex items-baseline justify-between gap-3 rounded-xl px-4 py-2.5"
          >
            <span class="truncate font-semibold">{{ row.label }}</span>
            <span class="shrink-0 text-sm text-muted-foreground tabular-nums">{{ plural(row.matches) }}</span>
          </li>
        </ul>
      </div>

      <div v-if="view.facts.length > 0" class="mt-auto grid grid-cols-2 gap-3 pt-6">
        <div v-for="fact in view.facts" :key="fact.label" class="wrapped-fact wrapped-surface rounded-xl px-4 py-3">
          <p class="text-[10px] font-medium uppercase tracking-widest text-muted-foreground">{{ fact.label }}</p>
          <p class="mt-1 text-lg font-bold leading-tight">{{ fact.value }}</p>
          <p v-if="fact.detail" class="mt-1 text-xs leading-snug text-balance text-muted-foreground">{{ fact.detail }}</p>
        </div>
      </div>

      <div v-if="view.visual?.kind === 'hours'" :class="view.facts.length > 0 ? 'mt-4' : 'mt-auto pt-6'">
        <div class="grid grid-cols-24 gap-0.5">
          <span
            v-for="(value, hour) in view.visual.byHour"
            :key="hour"
            class="h-5 rounded-sm"
            :class="hourShade(value, maxHour)"
            :title="`${hourText(hour)}: ${value}`"
          />
        </div>
        <div class="mt-1 flex justify-between text-[10px] text-muted-foreground">
          <span>midnight</span><span>noon</span><span>11pm</span>
        </div>
      </div>


      <WrappedActivity v-if="view.visual?.kind === 'days'" :by-day="view.visual.byDay" :year="bundle.year" class="mt-6" />

      <!-- The numbers card: an open row of figures under a hairline, no tiles. -->
      <div
        v-if="view.figures.length > 0 && view.visual?.kind === 'days'"
        class="mt-auto grid divide-x divide-white/10 border-t border-white/10 pt-6"
        :style="{ gridTemplateColumns: `repeat(${view.figures.length}, minmax(0, 1fr))` }"
      >
        <div v-for="figure in view.figures" :key="figure.label" class="wrapped-figure px-6 first:pl-0">
          <p class="text-5xl font-extrabold tracking-tight tabular-nums">{{ figure.value }}</p>
          <p class="mt-1 text-xs font-medium tracking-widest text-emerald-300/80 uppercase">{{ figure.label }}</p>
        </div>
      </div>

      <div v-else-if="view.figures.length > 0" class="mt-auto">
        <p v-if="view.figuresHeading" class="mb-2 text-sm font-medium text-muted-foreground">{{ view.figuresHeading }}</p>
        <div class="grid grid-cols-3 gap-3">
          <div v-for="figure in view.figures" :key="figure.label" class="wrapped-figure wrapped-surface rounded-xl px-5 py-4">
            <p class="text-4xl font-extrabold tracking-tight tabular-nums">{{ figure.value }}</p>
            <p class="mt-1 text-sm text-muted-foreground">{{ figure.label }}</p>
          </div>
        </div>
      </div>

      <p v-if="view.badge" class="wrapped-badge mt-3 text-center text-xs text-foreground/70">{{ view.badge }}</p>

    </div>

    <div v-if="hasArt && view.art" class="relative min-w-0">
      <CardImage
        :src="view.art.remote"
        :local="view.art.local"
        :custom="view.art.custom"
        :alt="view.title ?? view.label"
        class="absolute inset-0 size-full object-cover"
        @error="onArtError"
      />
      <div class="absolute inset-0 bg-gradient-to-r from-card via-card/30 to-transparent" aria-hidden="true" />
      <p
        v-if="view.credit"
        class="wrapped-credit absolute right-3 bottom-2 text-[10px] text-white/70 [text-shadow:_0_1px_3px_rgb(0_0_0_/_90%)]"
      >
        {{ view.credit }}
      </p>
    </div>
  </Card>
</template>

<style scoped>
/* Darkens the scene on the words' side so they read on any sky. */
.wrapped-scene-shade {
  background: linear-gradient(90deg, rgb(13 13 13 / 0.92) 0%, rgb(13 13 13 / 0.6) 50%, rgb(13 13 13 / 0) 85%);
}

/* The figure tiles' surface, so they sit on the numbers card's art. */
.wrapped-surface {
  border: 1px solid rgb(255 255 255 / 0.08);
  background: rgb(0 0 0 / 0.45);
  backdrop-filter: blur(6px);
}
</style>
