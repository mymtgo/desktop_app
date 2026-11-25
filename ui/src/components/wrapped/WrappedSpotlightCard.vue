<script setup lang="ts">
import { Medal, Trophy } from 'lucide-vue-next'
import { computed } from 'vue'
import CardImage from '@/components/CardImage.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'

/**
 * The spotlight layout (the MVP, the opening hands card, the sideboard hero,
 * comebacks): art faint behind everything, the name, one big number, a line
 * and the numbers, and the full card on the right when there is one. The
 * words come from `cardView`.
 */
const props = defineProps<{ view: CardView }>()

// The cabinet shows at most this many of each; more reads as "+N".
const SHELF = 8
const full = computed(() => props.view.artStrength === 'full' && !props.view.spotlight)

function shelf(total: number): { shown: number; more: number } {
  return { shown: Math.min(total, SHELF), more: Math.max(0, total - SHELF) }
}
</script>

<template>
  <Card
    class="relative grid h-full grid-rows-[minmax(0,1fr)] gap-0 overflow-hidden py-0"
    :class="view.spotlight ? 'grid-cols-[1.3fr_1fr]' : 'grid-cols-1'"
  >
    <CardImage
      v-if="view.art"
      :src="view.art.remote"
      :local="view.art.local"
      :custom="view.art.custom"
      alt=""
      class="wrapped-spotlight-art pointer-events-none absolute inset-0 size-full object-cover"
      :class="full ? '' : 'opacity-15'"
    />
    <!-- Full-strength art: dark behind the words on the left, the art clear on the right. -->
    <div
      v-if="full && view.art"
      class="pointer-events-none absolute inset-0 bg-linear-to-r from-black/90 via-black/55 to-black/0"
      aria-hidden="true"
    />

    <div class="relative flex min-h-0 min-w-0 flex-col p-8 [text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <h2 class="wrapped-title mt-2 line-clamp-2 text-3xl leading-tight font-extrabold">{{ view.title }}</h2>

      <div class="mt-6">
        <p class="flex items-baseline gap-3">
          <span class="wrapped-impact text-6xl leading-none font-extrabold tracking-tight tabular-nums">{{ view.big }}</span>
          <span v-if="view.bigLabel" class="text-sm text-muted-foreground">{{ view.bigLabel }}</span>
        </p>
        <p v-for="line in view.lines" :key="line" class="mt-3 max-w-md text-sm text-balance text-foreground/85">{{ line }}</p>
      </div>

      <div v-if="view.cabinet" class="wrapped-cabinet mt-auto flex flex-wrap items-end gap-x-10 gap-y-4 pt-6">
        <div v-if="view.cabinet.trophies > 0">
          <div class="flex items-end gap-1.5">
            <Trophy
              v-for="n in shelf(view.cabinet.trophies).shown"
              :key="n"
              class="wrapped-trophy size-10 text-amber-300 drop-shadow-[0_0_12px_rgb(251_191_36_/_55%)]"
              :stroke-width="1.75"
            />
            <span v-if="shelf(view.cabinet.trophies).more > 0" class="text-sm font-semibold text-amber-200">+{{ shelf(view.cabinet.trophies).more }}</span>
          </div>
          <p class="mt-2 text-sm text-foreground/70">
            <b class="font-extrabold text-foreground">{{ view.cabinet.trophies }}</b>
            {{ view.cabinet.trophies === 1 ? 'league trophy' : 'league trophies' }}
          </p>
        </div>
        <div v-if="view.cabinet.top8s > 0">
          <div class="flex items-end gap-1.5">
            <Medal
              v-for="n in shelf(view.cabinet.top8s).shown"
              :key="n"
              class="wrapped-medal size-8 text-slate-200 drop-shadow-[0_0_10px_rgb(226_232_240_/_40%)]"
              :stroke-width="1.75"
            />
            <span v-if="shelf(view.cabinet.top8s).more > 0" class="text-sm font-semibold text-slate-200">+{{ shelf(view.cabinet.top8s).more }}</span>
          </div>
          <p class="mt-2 text-sm text-foreground/70">
            <b class="font-extrabold text-foreground">{{ view.cabinet.top8s }}</b> {{ view.cabinet.top8s === 1 ? 'Top 8' : 'Top 8s' }}
          </p>
        </div>
      </div>

      <p
        v-else-if="view.figures.length > 0"
        class="flex items-baseline gap-x-3 text-sm whitespace-nowrap text-foreground/70"
        :class="view.runnersUp ? 'mt-5' : 'mt-auto pt-6'"
      >
        <template v-for="(figure, index) in view.figures" :key="figure.label">
          <span v-if="index > 0" class="text-foreground/30" aria-hidden="true">·</span>
          <span class="wrapped-figure"><b class="text-lg font-extrabold text-foreground tabular-nums">{{ figure.value }}</b> {{ figure.label }}</span>
        </template>
      </p>

      <div v-if="view.runnersUp" class="wrapped-runners-up mt-auto border-t border-white/10 pt-4">
        <p class="mb-2 text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.runnersUp.heading }}</p>
        <ul class="flex max-w-md flex-col gap-1.5 text-sm">
          <li v-for="row in view.runnersUp.rows" :key="row.name" class="wrapped-runner-up flex items-baseline justify-between gap-4">
            <span class="truncate text-foreground/85">{{ row.name }}</span>
            <span class="shrink-0 font-semibold tabular-nums">{{ row.value }}</span>
          </li>
        </ul>
      </div>
    </div>

    <div v-if="view.spotlight" class="relative flex min-h-0 min-w-0 items-center justify-center py-8 pr-8">
      <CardImage
        :src="view.spotlight.remote"
        :local="view.spotlight.local"
        :custom="view.spotlight.custom"
        :alt="view.title ?? view.label"
        class="wrapped-spotlight-image h-full max-h-full w-auto max-w-full rounded-[4.5%/3.3%] object-contain shadow-[0_24px_48px_-12px_rgba(0,0,0,0.95)]"
      />
    </div>

    <p
      v-if="view.credit"
      class="wrapped-credit absolute right-3 bottom-2 text-[10px] text-white/70 [text-shadow:_0_1px_3px_rgb(0_0_0_/_90%)]"
    >
      {{ view.credit }}
    </p>
  </Card>
</template>
