<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'

/**
 * The Limited card: its art clear on the right, the drafts as a big number,
 * then the favourite colours as large mana symbols and the best run.
 */
defineProps<{ view: CardView }>()
</script>

<template>
  <Card class="relative flex h-full flex-col gap-0 overflow-hidden py-0">
    <CardImage
      v-if="view.art"
      :src="view.art.remote"
      :local="view.art.local"
      :custom="view.art.custom"
      alt=""
      class="pointer-events-none absolute inset-0 size-full object-cover"
    />
    <div class="pointer-events-none absolute inset-0 bg-linear-to-r from-black/90 via-black/60 to-black/0" aria-hidden="true" />

    <div class="relative flex min-h-0 flex-1 flex-col p-8 [text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <h2 class="wrapped-title mt-2 text-3xl leading-tight font-extrabold">{{ view.title }}</h2>
      <p class="mt-6 flex items-baseline gap-3">
        <span class="text-6xl leading-none font-extrabold tracking-tight tabular-nums">{{ view.big }}</span>
        <span class="text-sm text-muted-foreground">{{ view.bigLabel }}</span>
      </p>

      <div v-if="view.limited" class="mt-auto flex items-end gap-12 pt-6">
        <div v-if="view.limited.pair" class="wrapped-limited-colours">
          <ManaColors v-if="view.limited.colors.length > 0" :colors="view.limited.colors" class="!text-4xl gap-2" />
          <p class="mt-3 text-sm text-foreground/70"><b class="font-extrabold text-foreground">{{ view.limited.pair }}</b> was your go-to</p>
        </div>
        <div v-if="view.limited.record" class="wrapped-limited-record">
          <p class="text-4xl leading-none font-extrabold tracking-tight text-emerald-300 tabular-nums">{{ view.limited.record }}</p>
          <p class="mt-3 text-sm text-foreground/70">your best run</p>
        </div>
      </div>
    </div>

    <p v-if="view.credit" class="wrapped-credit absolute right-3 bottom-2 text-[10px] text-white/70 [text-shadow:_0_1px_3px_rgb(0_0_0_/_90%)]">
      {{ view.credit }}
    </p>
  </Card>
</template>
