<script setup lang="ts">
import { computed, shallowRef, watch } from 'vue'
import type { ReplayWindowView } from '@/bindings'
import ReplayViewer from '@/replay-mtgr/ReplayViewer.vue'
import { mtgrCardImage, mtgrGames } from './mtgrReplay'

/**
 * An Arena game's replay (spec 4.4): the match's `.mtgr` text in the
 * vendored `.mtgr` viewer, on the requested game; the viewer's picker
 * switches games in place. Arena replays are never shared, so the viewer
 * gets no `shareUrl`.
 */
const props = defineProps<{ view: Exclude<ReplayWindowView, { kind: 'snapshot' | 'noRecord' }> }>()
/** The viewer's picker moved this window to another game of the record (its index). */
const emit = defineEmits<{ selectGame: [index: number] }>()

const games = computed(() => (props.view.kind === 'mtgr' ? mtgrGames(props.view.text) : []))
const cardImage = computed(() =>
  props.view.kind === 'mtgr' ? mtgrCardImage(props.view.cards, props.view.printings) : mtgrCardImage([]),
)
const gameIndex = shallowRef(props.view.kind === 'mtgr' ? props.view.gameIndex : 0)

function pick(index: number) {
  gameIndex.value = index
  emit('selectGame', index)
}

watch(
  () => props.view,
  (view) => {
    if (view.kind === 'mtgr') gameIndex.value = view.gameIndex
  },
)
</script>

<template>
  <ReplayViewer
    v-if="view.kind === 'mtgr' && games[gameIndex]"
    :key="gameIndex"
    :games="games"
    :game-index="gameIndex"
    :card-image="cardImage"
    @select-game="pick"
  />
  <main v-else class="flex min-h-0 flex-1 items-center justify-center p-6">
    <p v-if="view.kind === 'missingCardData'" class="text-sm text-muted-foreground">
      Some cards in this game aren't in card data yet. The replay will open once they are.
    </p>
    <p v-else class="text-sm text-destructive">This replay has no games to show.</p>
  </main>
</template>
