<script setup lang="ts">
import { Settings2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import LeagueOverlayCard from '@/components/overlay/LeagueOverlayCard.vue'
import type { PreviewSample } from '@/components/overlay/overlaySamples'
import EventCardSheet from '@/components/settings/EventCardSheet.vue'
import EventPreviewControls from '@/components/settings/EventPreviewControls.vue'
import { Button } from '@/components/ui/button'
import type { LeagueTheme, OverlayArtwork, OverlaySettingsView, OverlaySize, Settings } from '@/bindings'
import type { EventKind, LeagueDisplay } from '@/bindings'
import {
  PREVIEW_OPTIONS,
  firstPreview,
  previewCaption,
  previewState,
  withStrip,
} from '@/lib/overlaySettings'

// The event card on the Overlays page (event overlay spec 4.2): a 1:1 preview
// at the saved theme and size, with the League | Challenge switch and the
// grouped state picker, and the Customise slide-out (EventCardSheet) for the
// look and the parts. Where the card shows is EventOutputsCard.
const props = defineProps<{
  artwork: OverlayArtwork
  size: OverlaySize
  theme: LeagueTheme
  /** What the card shows (`overlay_league_display`), shared with the hosted overlay. */
  display: LeagueDisplay
  /** Minutes a finished run stays on the card (`overlay_finished_minutes`), shared with the hosted overlay. */
  finishedMinutes: number
  /** `overlayEventStrip`. */
  eventStrip: boolean
  view: OverlaySettingsView
  /** A theme, size or strip change is saving: the view's OBS sizes are old. */
  sizesUpdating: boolean
}>()

const emit = defineEmits<{
  change: [change: Partial<Settings>]
  /** The background was chosen or removed: the page reads again. */
  background: []
}>()

// One preview state for the page and the slide-out.
const previewKind = ref<EventKind>('league')
const previewSample = ref<PreviewSample>(firstPreview('league'))
// One fixed time for both previews: sample estimates never run down while the page stays open.
const previewNow = Date.now()
const preview = computed(() =>
  withStrip(
    previewState(previewSample.value, { ...props.view, artwork: props.artwork }, props.size, props.display, props.theme, previewNow),
    props.eventStrip,
  ),
)
const caption = computed(() => previewCaption(props.theme, props.size, props.artwork, props.sizesUpdating ? null : props.view.obsSizes[props.size]))

/** Shows a sample, switching the kind to the sample's. */
function showSample(sample: PreviewSample): void {
  previewKind.value = PREVIEW_OPTIONS.find((option) => option.value === sample)?.kind ?? previewKind.value
  previewSample.value = sample
}

const sheetOpen = ref(false)
let opener: HTMLElement | null = null

function openSheet(event: MouseEvent): void {
  opener = event.currentTarget as HTMLElement | null
  sheetOpen.value = true
}

// Focus goes back to whichever button opened the slide-out.
function returnFocus(event: Event): void {
  event.preventDefault()
  opener?.focus()
}
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="flex items-start justify-between gap-3">
      <div class="space-y-1">
        <h2 class="text-sm font-semibold">Event card</h2>
        <p class="text-xs text-muted-foreground">
          Your current league run or event. One card for the desktop window and the stream overlay.
        </p>
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        class="shrink-0"
        aria-haspopup="dialog"
        :aria-expanded="sheetOpen"
        @click="openSheet"
      >
        <Settings2 class="size-4" />
        Customise
      </Button>
    </div>

    <div class="flex flex-col gap-3 rounded-md bg-muted/30 p-3">
      <EventPreviewControls v-model:kind="previewKind" v-model:sample="previewSample" />
      <div class="max-w-full overflow-x-auto">
        <LeagueOverlayCard :state="preview" :now="previewNow" />
      </div>
      <button
        type="button"
        class="cursor-pointer self-start text-left text-xs text-muted-foreground underline decoration-white/20 underline-offset-[3px] hover:text-foreground"
        aria-haspopup="dialog"
        @click="openSheet"
      >
        {{ caption }}
      </button>
    </div>

    <EventCardSheet
      v-model:open="sheetOpen"
      v-model:preview-kind="previewKind"
      v-model:preview-sample="previewSample"
      :preview="preview"
      :now="previewNow"
      :artwork="artwork"
      :size="size"
      :theme="theme"
      :display="display"
      :finished-minutes="finishedMinutes"
      :event-strip="eventStrip"
      :view="view"
      :sizes-updating="sizesUpdating"
      @change="(change) => emit('change', change)"
      @background="emit('background')"
      @preview="showSample"
      @close-auto-focus="returnFocus"
    />
  </section>
</template>
