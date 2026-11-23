<script setup lang="ts">
import { Play } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { commands, type GameDetailView, type ManualEditingView } from '@/bindings'
import { Button } from '@/components/ui/button'
import EditOpeningHandDialog from '@/components/matches/EditOpeningHandDialog.vue'
import EditSideboardDialog from '@/components/matches/EditSideboardDialog.vue'
import GameLogDialog from '@/components/matches/GameLogDialog.vue'
import OpeningHand from '@/components/matches/OpeningHand.vue'
import SideboardChanges from '@/components/matches/SideboardChanges.vue'
import { useClearingMessage } from '@/composables/useClearingMessage'
import { showReplayButton } from '@/lib/matchDetail'

/**
 * One game's panels (0.x `MatchGame.vue`): the opening hand with the game
 * log and replay in its header, the mulliganed hands and, from game 2, the
 * sideboard changes. The game's facts sit on its tile above. No clocks
 * (no data). "Replay" opens the game in the replay window when the match was
 * not entered by hand or read from MTGO's history and the game has frames. A
 * manual match's hand and sideboard changes open their edit dialogs.
 * `imported` is an imported match with no game detail: it has no hand or
 * sideboard panels.
 */
const props = withDefaults(
  defineProps<{
    token: string
    /** The match's source: `log`, `imported`, `manual` or `history`. */
    source: string
    game: GameDetailView
    imported: boolean
    manual?: boolean
    manualEditing?: ManualEditingView | null
  }>(),
  { manual: false, manualEditing: null },
)

const handDialog = ref<InstanceType<typeof EditOpeningHandDialog> | null>(null)
const sideboardDialog = ref<InstanceType<typeof EditSideboardDialog> | null>(null)

/** This game's maindeck, with its recorded sideboard changes applied (worked out by the app). */
const effectiveMains = computed(
  () => props.manualEditing?.games.find((g) => g.number === props.game.number)?.mains ?? [],
)
const editable = computed(() => props.manual && props.manualEditing !== null)


const replayable = computed(() => showReplayButton({ source: props.source }, props.game))
// The error goes away by itself, on the next click and with another game.
const { message: replayError, show: showReplayError, clear: clearReplayError } = useClearingMessage()
watch(() => props.game.id, clearReplayError)

async function openReplay() {
  clearReplayError()
  const result = await commands.openReplayWindow(props.game.id)
  if (result.status === 'error') showReplayError(result.error)
}
</script>

<template>
  <div class="flex flex-col gap-5">
    <template v-if="!imported">
      <OpeningHand :game="game" :manual="editable" @edit="handDialog?.open()">
        <template #actions>
          <GameLogDialog v-if="game.hasGameLog" :token="token" :game-number="game.number" />
          <Button v-if="replayable" variant="ghost" size="sm" class="h-7 px-2 text-xs" @click="openReplay">
            <Play :size="12" />
            Replay
          </Button>
          <span v-if="replayError" class="text-xs text-destructive">{{ replayError }}</span>
        </template>
      </OpeningHand>
      <SideboardChanges
        v-if="game.number > 1"
        :changes="game.sideboardChanges"
        :manual="editable"
        @edit="sideboardDialog?.open()"
      />
    </template>
    <!-- An imported match has no hand panel: its game log sits on its own. -->
    <div v-else-if="game.hasGameLog || replayable" class="flex items-center justify-end gap-1">
      <GameLogDialog v-if="game.hasGameLog" :token="token" :game-number="game.number" />
      <Button v-if="replayable" variant="ghost" size="sm" class="h-7 px-2 text-xs" @click="openReplay">
        <Play :size="12" />
        Replay
      </Button>
      <span v-if="replayError" class="text-xs text-destructive">{{ replayError }}</span>
    </div>

    <EditOpeningHandDialog v-if="editable" ref="handDialog" :token="token" :game="game" :mains="effectiveMains" />
    <EditSideboardDialog
      v-if="editable && manualEditing && game.number > 1"
      ref="sideboardDialog"
      :token="token"
      :game="game"
      :mains="manualEditing.mains"
      :sideboard="manualEditing.sideboard"
    />
  </div>
</template>
