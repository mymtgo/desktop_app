<script setup lang="ts">
import { computed } from 'vue'
import type { OpponentArchetypeView } from '@/bindings'
import CardHoverPreview from '@/components/cards/CardHoverPreview.vue'

/**
 * An opponent archetype's name. A commander archetype previews its
 * commander card on hover (partners both) through the app's card hover;
 * any other archetype is its name only.
 */
const props = defineProps<{ archetype: OpponentArchetypeView }>()

const first = computed(() => props.archetype.commanderCards[0] ?? null)
const partners = computed(() => props.archetype.commanderCards.slice(1))
</script>

<template>
  <CardHoverPreview v-if="first" :image="first.image" :local-image="first.localImage" :name="first.name" :partners="partners">
    <span>{{ archetype.name }}</span>
  </CardHoverPreview>
  <span v-else>{{ archetype.name }}</span>
</template>
