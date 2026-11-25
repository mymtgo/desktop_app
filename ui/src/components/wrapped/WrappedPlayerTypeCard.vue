<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'

/**
 * The player type: the type's name and line on the left, and on the right
 * the player's own card, in the app's card style (not a Magic frame), with
 * the type's art and what earned it written in its text box.
 */
defineProps<{ view: CardView; year: number }>()
</script>

<template>
  <Card class="relative grid h-full grid-cols-[1.2fr_1fr] grid-rows-[minmax(0,1fr)] gap-0 overflow-hidden py-0">
    <CardImage
      v-if="view.persona"
      :src="view.persona.art.remote"
      :local="null"
      :custom="null"
      alt=""
      class="pointer-events-none absolute inset-0 size-full scale-110 object-cover opacity-20 blur-xl"
    />

    <div class="relative flex min-w-0 flex-col justify-center p-8">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <h2 class="wrapped-title mt-3 text-5xl leading-tight font-extrabold">{{ view.title }}</h2>
      <p v-for="line in view.lines" :key="line" class="mt-4 max-w-md text-lg leading-snug text-balance text-foreground/85">{{ line }}</p>
    </div>

    <div class="relative flex min-h-0 items-center justify-center py-8 pr-8">
      <!-- The player's card, in the app's own card style: the name plate, the art and a text box. -->
      <div
        class="wrapped-persona flex aspect-[5/7] h-full max-h-full flex-col overflow-hidden rounded-xl border border-white/10 bg-card shadow-[0_24px_48px_-12px_rgba(0,0,0,0.95)]"
      >
        <div class="flex min-h-0 flex-1 flex-col">
          <p class="truncate px-3 py-2 text-sm font-bold">{{ view.title }}</p>
          <div class="relative mx-2 min-h-0 flex-[1.6] overflow-hidden rounded-md">
            <CardImage
              v-if="view.persona"
              :src="view.persona.art.remote"
              :local="null"
              :custom="null"
              alt=""
              class="absolute inset-0 size-full object-cover"
            />
          </div>
          <p class="px-3 pt-2 text-[10px] font-medium tracking-widest text-muted-foreground uppercase">MyMTGO player · {{ year }}</p>
          <ul class="flex min-h-0 flex-1 flex-col divide-y divide-white/5 px-3 pt-1 text-sm">
            <li v-for="pill in view.pills" :key="pill" class="wrapped-persona-line truncate py-1.5 font-semibold">{{ pill }}</li>
          </ul>
          <p v-if="view.persona" class="truncate px-3 pb-2 text-[9px] text-muted-foreground">{{ view.persona.credit }}</p>
        </div>
      </div>
    </div>
  </Card>
</template>
