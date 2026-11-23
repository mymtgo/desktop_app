<script setup lang="ts">
import type { OverlayArchetypeOption, OverlayGame, OverlayOpponent } from '@/bindings'
import { computed } from 'vue'
import { WAITING_FOR_OPPONENT } from '@/lib/overlay'
import ArchetypeSelect from './ArchetypeSelect.vue'
import GameStartStrip from './GameStartStrip.vue'

/**
 * The opponent header (0.x `OpponentHeader`): their name, the head-to-head
 * count ("Met {n}× · won {w}", a count, not a score), and the archetype
 * picker with where the archetype came from. Above the name, once logged,
 * the current game's play or draw and kept hands. The strip drags the
 * window.
 */
const props = defineProps<{
  opponent: OverlayOpponent | null
  game: OverlayGame | null
  options: OverlayArchetypeOption[] | null
}>()

const emit = defineEmits<{ select: [option: OverlayArchetypeOption]; open: [] }>()

const showStart = computed(
  () => props.game !== null && (props.game.onPlay !== null || props.game.localHand !== null || props.game.opponentHand !== null),
)
</script>

<template>
  <div data-tauri-drag-region class="border-b border-border">
    <GameStartStrip
      v-if="props.opponent && props.game && showStart"
      :game="props.game"
      class="border-b border-border px-3 py-1.5"
    />

    <div data-tauri-drag-region class="flex flex-col gap-1.5 px-3 py-2">
      <template v-if="props.opponent">
        <div data-tauri-drag-region class="flex items-baseline justify-between gap-2">
          <span data-tauri-drag-region class="truncate text-sm font-semibold">{{ props.opponent.username }}</span>
          <span v-if="props.opponent.headToHead" data-tauri-drag-region class="shrink-0 text-xs text-muted-foreground">
            {{ props.opponent.headToHead }}
          </span>
        </div>

        <ArchetypeSelect
          :options="props.options"
          :current-id="props.opponent.archetypeId"
          :current-uuid="props.opponent.archetypeUuid"
          :current-name="props.opponent.archetypeName"
          :source-label="props.opponent.sourceLabel"
          :unreachable="props.opponent.unreachable"
          @select="emit('select', $event)"
          @open="emit('open')"
        />
      </template>

      <p v-else data-tauri-drag-region class="py-1 text-center text-xs text-muted-foreground">{{ WAITING_FOR_OPPONENT }}</p>
    </div>
  </div>
</template>
