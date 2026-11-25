<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import WrappedCard from '@/components/wrapped/WrappedCard.vue'
import WrappedCarousel from '@/components/wrapped/WrappedCarousel.vue'
import { FIRST_YEAR_NOTE } from '@/lib/wrapped/copy'
import { useWrapped } from '@/lib/wrapped/useWrapped'

/**
 * MyMTGO Wrapped (Wrapped spec): the player's year as a story of cards. A
 * player's first Wrapped says so once, at the foot of the page under every
 * card.
 */
const router = useRouter()
const { bundle } = useWrapped()

const cards = computed(() => bundle.value?.cards ?? [])

function close() {
  void router.push({ name: 'dashboard' })
}
</script>

<template>
  <div class="relative">
    <WrappedCarousel v-if="bundle && cards.length > 0" :cards="cards" :year="bundle.year" @close="close">
      <template #card="{ card, restart }">
        <WrappedCard :card="card" :bundle="bundle" @restart="restart" />
      </template>
    </WrappedCarousel>
    <div v-else class="flex min-h-[calc(100vh-7rem)] flex-col items-center justify-center gap-2 text-center">
      <p class="text-lg font-semibold">Wrapped isn't ready yet</p>
      <RouterLink :to="{ name: 'dashboard' }" class="text-sm text-muted-foreground underline-offset-4 hover:underline">
        Back to the dashboard
      </RouterLink>
    </div>

    <p
      v-if="bundle?.firstYear && cards.length > 0"
      class="wrapped-first-year absolute inset-x-0 bottom-4 mx-auto max-w-xl px-4 text-center text-xs text-muted-foreground"
    >
      {{ FIRST_YEAR_NOTE }}
    </p>
  </div>
</template>
