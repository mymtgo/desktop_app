<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { commands, events, type ReplayWindowView } from '@/bindings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import { previousBaseline, requestedGame, switchedGame } from './entry'
import MtgrReplay from './MtgrReplay.vue'
import ReplayEmptyState from './ReplayEmptyState.vue'
import ReplayShareButton from './share/ReplayShareButton.vue'
import ReplayViewer from './ReplayViewer.vue'
import type { ReplayFrame } from './types'

/**
 * The replay window's page: loads `replay_view` for the game it shows,
 * switches games in place when the app pushes `replay-game-changed` (another
 * game's Replay button; a click during the page's own load is asked for once
 * the listener is registered) or the viewer's game picker is used. The viewer is
 * the package's, mounted as 0.x's `games/Show.vue` did, with `onSelectGame`
 * in place of its Inertia links; the viewer shows its own empty state. An
 * Arena or Endstep game shows its `.mtgr` record in the vendored `.mtgr`
 * viewer (`MtgrReplay`), or why it has none. A `null` view (the game was
 * deleted) and a `noRecord` view (the game is stored but has no replay) each
 * show the empty state with their own message.
 */
const props = defineProps<{ initialGame: number | null }>()

const gameId = ref<number | null>(props.initialGame)
const view = ref<ReplayWindowView | null>(null)
const error = ref<string | null>(props.initialGame === null ? 'No game to replay.' : null)
const loading = ref(false)

// The in-window picker of an Arena or Endstep record moved off the game
// `gameId` names, so asking for that game again has to open it.
const moved = ref(false)

// Only the newest request's answer is shown when games switch quickly.
let request = 0

async function load(id: number) {
  const mine = ++request
  moved.value = false
  loading.value = true
  const result = await commands.replayView(id)
  if (mine !== request) return
  loading.value = false
  if (result.status === 'ok') {
    view.value = result.data
    error.value = null
  } else {
    view.value = null
    error.value = result.error
  }
}

function selectGame(id: number) {
  const next = switchedGame(gameId.value, id, moved.value)
  if (next === null) return
  // The same game again: no change for the watcher to see, so load it here.
  if (next === gameId.value) void load(next)
  else gameId.value = next
}

watch(
  gameId,
  (id) => {
    if (id !== null) void load(id)
  },
  { immediate: true },
)

const listening = useTauriEvents(() => [events.replayGameChanged.listen((event) => selectGame(event.payload.gameId))])
onMounted(async () => {
  if (!(await listening.ready)) return
  // A Replay click while this page loaded pushed an event nobody heard:
  // ask the app once, now that the listener is registered.
  const next = requestedGame(gameId.value, await commands.replayRequestedGame())
  if (next !== null && listening.isMounted()) selectGame(next)
})

/** An MTGO game's snapshot view, or null. */
const snapshot = computed(() => (view.value?.kind === 'snapshot' ? view.value : null))
/** An Arena or Endstep game's view (its record, or missing card data), or null. */
const arena = computed(() => (view.value && view.value.kind !== 'snapshot' && view.value.kind !== 'noRecord' ? view.value : null))
/** Why there is nothing to show: the game is gone, or it has no record. */
const emptyMessage = computed(() =>
  view.value?.kind === 'noRecord' ? 'No replay was recorded for this game.' : 'This game is no longer stored.',
)
/** The frames as the viewer types them; `replay_view` sends their content untyped. */
const frames = computed(() => (snapshot.value?.frames ?? []) as ReplayFrame[])
const previous = computed(() => previousBaseline(snapshot.value?.previousSideboard ?? null))
</script>

<template>
  <div class="flex h-screen flex-col overflow-hidden bg-background text-foreground">
    <ReplayViewer
      v-if="snapshot"
      :key="snapshot.gameId"
      :frames="frames"
      :log="snapshot.log"
      :won="snapshot.won"
      :game-id="snapshot.gameId"
      :match-games="snapshot.matchGames"
      :sideboard="snapshot.sideboard"
      :previous-sideboard="previous"
      :archetypes="snapshot.archetypes"
      :on-select-game="selectGame"
    >
      <template v-if="snapshot.share.shareable" #actions>
        <ReplayShareButton :game-id="snapshot.gameId" />
      </template>
    </ReplayViewer>
    <MtgrReplay v-else-if="arena" :key="gameId ?? undefined" :view="arena" @select-game="moved = true" />
    <ReplayEmptyState
      v-else-if="!error && !loading"
      title="No replay to show"
      :message="emptyMessage"
    />
    <main v-else class="flex min-h-0 flex-1 items-center justify-center p-6">
      <p v-if="error" class="text-sm text-destructive">{{ error }}</p>
      <p v-else class="text-sm text-muted-foreground">Loading…</p>
    </main>
  </div>
</template>
