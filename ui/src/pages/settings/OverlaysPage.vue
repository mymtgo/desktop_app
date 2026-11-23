<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import DraftNotesCard from '@/components/settings/DraftNotesCard.vue'
import GameOverlayCard from '@/components/settings/GameOverlayCard.vue'
import EventCardSettings from '@/components/settings/EventCardSettings.vue'
import EventOutputsCard from '@/components/settings/EventOutputsCard.vue'
import { commands, events, type OverlaySettingsView, type Settings } from '@/bindings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { errorMessage, unwrap } from '@/lib/commands'
import { cardsAvailability, changesSize, readOrder, serialSaves, sizesUpdating as sizesOld, withChange } from '@/lib/overlaySettings'
import { useSettings } from '@/lib/queries'

// 0.x settings/Overlays (overlay spec 11): four cards in 0.x's order. The
// settings save through `save_settings`; the overlay worker then opens or
// closes the windows, publishes or clears the hosted overlay and resizes
// the league window. The page's other state is `overlay_settings_page`,
// read again when the account, the card or the background changes; the
// publish state follows `overlay-publish-changed` pushes.
const { data: settings } = useSettings()
const { save } = useSaveSettings()

const view = ref<OverlaySettingsView | null>(null)
const problem = ref<string | null>(null)
const available = computed(() => (view.value ? cardsAvailability(view.value) : null))

// Read the page again after the saves: the OBS sizes follow the theme and
// the strip switch, and a strip change moves no card (no
// `league-overlay-changed` push).
const saves = serialSaves(saveChange, load)
// The settings as the user set them: the saved ones plus every change still
// saving, so the controls answer at once and are never disabled meanwhile.
const shown = computed(() => {
  if (!settings.value) return undefined
  const unsaved = saves.unsaved.value
  if (!unsaved || !view.value) return settings.value
  return withChange(settings.value, unsaved, view.value.overlayWindowsAvailable) ?? settings.value
})
// The view's OBS sizes are old until a size change is saved and a read asked
// after it lands (a failed read leaves them updating until the next one).
const reads = readOrder()
const sizesSavedAtRead = ref(0)
const sizesUpdating = computed(() => sizesOld(saves.unsaved.value, reads.applied.value, sizesSavedAtRead.value))
const artwork = computed(() => shown.value?.overlayArtwork ?? view.value?.artwork ?? 'deck')

async function load() {
  const read = reads.start()
  try {
    const next = unwrap(await commands.overlaySettingsPage())
    // The newest read that lands is kept; an older one never replaces it.
    if (reads.land(read)) view.value = next
    problem.value = null
  } catch (error) {
    problem.value = errorMessage(error)
  }
}

// Built from the latest saved settings when its turn comes.
async function saveChange(next: Partial<Settings>): Promise<boolean> {
  if (!settings.value || !view.value) return false
  const toSave = withChange(settings.value, next, view.value.overlayWindowsAvailable)
  if (!toSave) return true
  const saved = await save(toSave)
  if (saved && changesSize(next)) sizesSavedAtRead.value = reads.issued()
  return saved
}

function change(next: Partial<Settings>): void {
  if (!settings.value || !view.value) return
  void saves.change(next)
}

const listening = useTauriEvents(() => [
  events.overlayPublishChanged.listen((event) => {
    if (view.value) view.value = { ...view.value, publish: event.payload }
  }),
  events.apiChanged.listen(() => void load()),
  events.leagueOverlayChanged.listen(() => void load()),
])

onMounted(async () => {
  if (!(await listening.ready)) return
  await load()
})
</script>

<template>
  <div class="flex flex-col gap-3">
    <p v-if="problem" class="text-sm text-destructive">{{ problem }}</p>

    <template v-if="shown && view && available">
      <EventCardSettings
        :artwork="artwork"
        :size="shown.overlaySize"
        :theme="shown.overlayLeagueTheme"
        :display="shown.overlayLeagueDisplay"
        :finished-minutes="shown.overlayFinishedMinutes"
        :event-strip="shown.overlayEventStrip"
        :view="view"
        :sizes-updating="sizesUpdating"
        @change="change"
        @background="load"
      />
      <EventOutputsCard
        :window-enabled="shown.leagueWindow"
        :window-disabled="available.windowsDisabled"
        :note="available.note"
        :enabled="shown.overlayPublish"
        :url="view.publicUrl"
        :size="shown.overlaySize"
        :theme="shown.overlayLeagueTheme"
        :obs-sizes="sizesUpdating ? null : view.obsSizes"
        :strip="shown.overlayEventStrip"
        :publish="view.publish"
        :signed-in="view.signedIn"
        :disabled="available.publishDisabled"
        @change="change"
      />
      <GameOverlayCard
        :settings="shown"
        :note="available.note"
        :disabled="available.windowsDisabled"
        @change="change"
      />
      <DraftNotesCard
        :enabled="shown.draftNotesWindow"
        :note="available.note"
        :disabled="available.windowsDisabled"
        @change="change"
      />
    </template>
  </div>
</template>
