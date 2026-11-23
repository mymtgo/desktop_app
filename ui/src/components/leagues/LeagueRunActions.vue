<script setup lang="ts">
import { computed, ref } from 'vue'
import type { PlatformView, RunStateView } from '@/bindings'
import DeleteLeagueDialog from '@/components/leagues/DeleteLeagueDialog.vue'
import DropLeagueDialog from '@/components/leagues/DropLeagueDialog.vue'
import LeagueContextMenu from '@/components/leagues/LeagueContextMenu.vue'
import LeagueNotes from '@/components/leagues/LeagueNotes.vue'
import LinkMatchesDialog from '@/components/leagues/LinkMatchesDialog.vue'
import { useLeagueManage } from '@/composables/useLeagueManage'
import { canDeleteLeague, LEAGUE_ROUNDS } from '@/lib/leagues'

/**
 * A run's "..." menu with the dialogs it opens (notes, Drop, Delete, Add
 * matches), for every run kind (league self-management spec 5.1): the league
 * card and the Limited index row share it so they cannot drift. Copy
 * screenshot and copy summary need the host's data, so they are emitted up.
 * `addMatches` and `editNotes` are exposed for the host's own buttons.
 */
const props = withDefaults(
  defineProps<{
    runKey: string
    matchCount: number
    /** A manual league: Add matches stops at five matches. */
    manual: boolean
    /** A limited run: the Add matches dialog lists matches of its set. */
    limited: boolean
    /** The player's status; `null` is Automatic. */
    userState: RunStateView | null
    /** The fold's state. */
    autoState: RunStateView
    notes: string | null
    /** Offer the two copy items (false where there is nothing to copy). */
    shareable?: boolean
    /** Offer "Set status" alone: an Arena run takes no other edit. */
    statusOnly?: boolean
    /** The run's game, for the Drop dialog's wording. */
    platform?: PlatformView
    disabled?: boolean
  }>(),
  { shareable: true, statusOnly: false, platform: 'mtgo', disabled: false },
)

const emit = defineEmits<{ screenshot: []; copySummary: [] }>()

const { setStatus } = useLeagueManage()

const notesDialog = ref<InstanceType<typeof LeagueNotes> | null>(null)
const dropDialog = ref<InstanceType<typeof DropLeagueDialog> | null>(null)
const deleteDialog = ref<InstanceType<typeof DeleteLeagueDialog> | null>(null)
const linkDialog = ref<InstanceType<typeof LinkMatchesDialog> | null>(null)

/** Delete is offered once the shown state (the player's status, else the fold's; spec 3.2) is not Active (spec 4.4). */
const canDelete = computed(() => canDeleteLeague({ state: props.userState ?? props.autoState }))
/** A manual league stops at five matches; log, imported and limited runs have no cap (spec 4.1). */
const canAddMatches = computed(() => !props.manual || props.matchCount < LEAGUE_ROUNDS)

function editNotes() {
  notesDialog.value?.openForRun(props.runKey, props.notes)
}

/** "Set status": Dropped asks first (it does not drop the player in the game); the rest apply at once. */
function chooseStatus(status: RunStateView | null) {
  if (status === 'dropped') {
    dropDialog.value?.openForRun(props.runKey)
  } else {
    void setStatus(props.runKey, status)
  }
}

function deleteLeague() {
  deleteDialog.value?.openForRun(props.runKey, props.matchCount)
}

function addMatches() {
  linkDialog.value?.openForRun(props.runKey, props.matchCount, props.manual, props.limited)
}

defineExpose({ editNotes, addMatches })
</script>

<template>
  <LeagueContextMenu
    :can-delete="canDelete"
    :can-add-matches="canAddMatches"
    :user-state="userState"
    :auto-state="autoState"
    :shareable="shareable"
    :status-only="statusOnly"
    :disabled="disabled"
    @screenshot="emit('screenshot')"
    @copy-summary="emit('copySummary')"
    @edit-notes="editNotes"
    @add-matches="addMatches"
    @set-status="chooseStatus"
    @delete="deleteLeague"
  />
  <LeagueNotes ref="notesDialog" />
  <DropLeagueDialog ref="dropDialog" :platform="platform" />
  <DeleteLeagueDialog ref="deleteDialog" />
  <LinkMatchesDialog ref="linkDialog" />
</template>
