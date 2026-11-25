<script setup lang="ts">
import { RotateCcw } from 'lucide-vue-next'
import { computed } from 'vue'
import CardImage from '@/components/CardImage.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'
import { setReleases } from '@/lib/wrapped/sets'

/**
 * The closing card: the year's set panels from the opening card again,
 * faint behind, so the story ends where it started; the goodbye, the
 * year's totals and "Watch again" in the middle.
 */
const props = defineProps<{ view: CardView; year: number }>()
const emit = defineEmits<{ restart: [] }>()

const sets = computed(() => setReleases(props.year))

function artUrl(set: string, number: string): string {
  return `https://api.scryfall.com/cards/${set}/${number}?format=image&version=art_crop`
}
</script>

<template>
  <Card class="relative flex h-full items-center justify-center gap-0 overflow-hidden py-0">
    <div v-if="sets.length > 0" class="wrapped-wrap-sets pointer-events-none absolute inset-0 flex opacity-30" aria-hidden="true">
      <div v-for="release in sets" :key="release.code" class="relative min-w-0 flex-1">
        <CardImage :src="artUrl(release.art.set, release.art.number)" :local="null" :custom="null" alt="" class="absolute inset-0 size-full object-cover" />
      </div>
    </div>
    <div class="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_at_center,rgb(10_10_10_/_0.92)_0%,rgb(10_10_10_/_0.75)_45%,rgb(10_10_10_/_0.2)_100%)]" aria-hidden="true" />

    <div class="relative flex flex-col items-center p-8 text-center">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <h2 class="wrapped-title mt-3 text-5xl leading-tight font-extrabold">{{ view.title }}</h2>
      <p v-for="line in view.lines" :key="line" class="mt-3 text-lg text-foreground/80">{{ line }}</p>

      <p v-if="view.recap.length > 0" class="wrapped-recap mt-8 flex flex-wrap items-baseline justify-center gap-x-4 gap-y-1 text-base text-foreground/70">
        <template v-for="(stat, index) in view.recap" :key="stat.label">
          <span v-if="index > 0" class="text-foreground/30" aria-hidden="true">·</span>
          <span><b class="text-2xl font-extrabold text-foreground tabular-nums">{{ stat.value }}</b> {{ stat.label }}</span>
        </template>
      </p>

      <button
        type="button"
        class="mt-8 flex items-center gap-2 rounded-full border border-white/15 bg-white/10 px-5 py-2 text-sm font-medium backdrop-blur hover:bg-white/20"
        @click="emit('restart')"
      >
        <RotateCcw class="size-4" /> Watch again
      </button>
    </div>
  </Card>
</template>
