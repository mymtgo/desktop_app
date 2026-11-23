<script setup lang="ts">
import { computed, useTemplateRef } from 'vue'
import type { LeagueOverlayState } from '@/bindings'
import { useOverlayDeckLabel } from '@/composables/useOverlayDeckLabel'
import { deckTitle, labelDeckId } from '@/lib/leagueOverlayCard'

/**
 * The inline input for the deck's label (overlay spec 9.2), saved 500 ms
 * after typing, shared by the league card themes. With no deck to label it
 * shows the plain title; `readonly` shows the label (or the title) as plain
 * text in the same box. The parent sets the font through `class`, which
 * lands on the root element.
 */
const props = defineProps<{ state: LeagueOverlayState; readonly?: boolean }>()

const labelInput = useTemplateRef<HTMLInputElement>('labelInput')
const labelDeck = computed(() => labelDeckId(props.state))
const title = computed(() => deckTitle(props.state))
const { label: customLabel, setLabel } = useOverlayDeckLabel(
  labelDeck,
  computed(() => props.state.deck?.label ?? null),
)

function onLabelInput(event: Event): void {
  setLabel((event.target as HTMLInputElement).value)
}

function blurLabel(): void {
  labelInput.value?.blur()
}
</script>

<template>
  <input
    v-if="labelDeck !== null && !readonly"
    ref="labelInput"
    type="text"
    :value="customLabel"
    :placeholder="title"
    spellcheck="false"
    class="min-w-0 truncate border-0 bg-transparent p-0 outline-none select-text [text-shadow:inherit] placeholder:text-white placeholder:opacity-100 focus:rounded focus:bg-white/10 focus:px-1"
    @input="onLabelInput"
    @keydown.enter.prevent="blurLabel"
    @keydown.escape.prevent="blurLabel"
  />
  <div v-else class="truncate">{{ (readonly && customLabel) || title }}</div>
</template>
