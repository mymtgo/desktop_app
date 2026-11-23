<script setup lang="ts">
import { useCardImage } from '@/lib/cards'

// The one card image (cards spec section 7): the downloaded file through
// the `card` scheme when `local` is set, else `src`; on a load error it
// swaps to `src` once. A deck's own cover image (`custom`, through the
// `deckcover` scheme) comes first and falls back the same way. Classes and
// attributes fall through to the img.
const props = defineProps<{ src: string | null; local?: string | null; custom?: string | null; alt: string }>()
const { current, onError } = useCardImage(props)
</script>

<template>
  <img v-if="current" :src="current" :alt="alt" @error="onError" />
</template>
