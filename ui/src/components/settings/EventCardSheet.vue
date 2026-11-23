<script setup lang="ts">
import { computed, ref } from 'vue'
import LeagueOverlayCard from '@/components/overlay/LeagueOverlayCard.vue'
import type { PreviewSample } from '@/components/overlay/overlaySamples'
import SegmentedControl from '@/components/SegmentedControl.vue'
import EventPreviewControls from '@/components/settings/EventPreviewControls.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import {
  commands,
  type LeagueGames,
  type LeagueTheme,
  type OverlayArtwork,
  type OverlaySettingsView,
  type OverlaySize,
  type Settings,
} from '@/bindings'
import { errorMessage } from '@/lib/commands'
import type { EventKind, LeagueDisplay, LeagueOverlayState } from '@/bindings'
import {
  ARTWORK_OPTIONS,
  GAMES_OPTIONS,
  KIND_TAGS,
  MAX_FINISHED_MINUTES,
  MIN_FINISHED_MINUTES,
  SHOW_OPTIONS,
  SIZE_OPTIONS,
  STRIP_SWITCH_HINT,
  THEME_OPTIONS,
  cardSize,
  sheetPreviewScale,
  sizeHint,
  stripHeight,
  typedFinishedMinutes,
  usesClassicOptions,
  type ShowOption,
} from '@/lib/overlaySettings'

/**
 * The event card's slide-out (event overlay spec 4.3): a modal sheet over the
 * dimmed app; a click outside, Escape and the X close it. Pinned at the top:
 * its own copy of the preview pickers and the preview, sharing the page's
 * preview state. Below, every look and part setting, each saved on change.
 * Whatever an option reveals sits directly beneath it (spec 4.1).
 */
const props = defineProps<{
  /** The page preview's state: the same card and sample. */
  preview: LeagueOverlayState
  /** The page preview's fixed time: relative sample times never tick. */
  now: number
  artwork: OverlayArtwork
  size: OverlaySize
  theme: LeagueTheme
  display: LeagueDisplay
  finishedMinutes: number
  /** `overlayEventStrip`. */
  eventStrip: boolean
  view: OverlaySettingsView
  /** A theme, size or strip change is saving: the view's OBS sizes are old. */
  sizesUpdating: boolean
}>()

const open = defineModel<boolean>('open', { required: true })
/** The page preview's kind and state (shared). */
const previewKind = defineModel<EventKind>('previewKind', { required: true })
const previewSample = defineModel<PreviewSample>('previewSample', { required: true })

const emit = defineEmits<{
  change: [change: Partial<Settings>]
  /** The background was chosen or removed: the page reads again. */
  background: []
  /** A one-kind part was switched: preview a state that shows it. */
  preview: [sample: PreviewSample]
  /** The sheet closed: the opener takes focus back. */
  closeAutoFocus: [event: Event]
}>()

/** The sheet is 420 px wide with 24 px padding on each side. */
const PREVIEW_WIDTH = 372

const card = computed(() => cardSize(props.theme, props.size))
const scale = computed(() => sheetPreviewScale(card.value.width, PREVIEW_WIDTH))
const previewBox = computed(() => {
  const height = card.value.height + (props.preview.other ? stripHeight(props.theme, props.size) : 0)
  return { width: card.value.width * scale.value, height: height * scale.value }
})
const obs = computed(() => (props.sizesUpdating ? null : props.view.obsSizes[props.size]))

// Opening focuses the chosen kind (reka would focus the first button,
// League, even while Challenge is chosen).
function focusChosenKind(event: Event): void {
  const chosen = (event.target as HTMLElement | null)?.querySelector?.<HTMLElement>('[aria-pressed="true"]')
  if (!chosen) return
  event.preventDefault()
  chosen.focus()
}

function setShown(option: ShowOption, on: boolean): void {
  emit('change', { overlayLeagueDisplay: { ...props.display, [option.key]: on } })
  if (option.preview) emit('preview', option.preview)
}

function setGames(games: LeagueGames): void {
  emit('change', { overlayLeagueDisplay: { ...props.display, games } })
}

// Saved when the box is left or Enter is pressed, not on every key. A blank
// or unreadable box shows the stored value again.
function setFinishedMinutes(event: Event): void {
  const box = event.target as HTMLInputElement
  const minutes = typedFinishedMinutes(box.value)
  if (minutes === null || minutes === props.finishedMinutes) {
    box.value = String(props.finishedMinutes)
    return
  }
  box.value = String(minutes)
  emit('change', { overlayFinishedMinutes: minutes })
}

const choosing = ref(false)
const failure = ref<string | null>(null)

async function changeBackground(send: () => Promise<{ status: 'ok' } | { status: 'error'; error: { message: string } }>) {
  choosing.value = true
  failure.value = null
  try {
    const answer = await send()
    if (answer.status === 'error') failure.value = answer.error.message
    else emit('background')
  } catch (error) {
    failure.value = errorMessage(error)
  } finally {
    choosing.value = false
  }
}

function chooseBackground() {
  return changeBackground(() => commands.chooseOverlayBackground())
}

function removeBackground() {
  return changeBackground(() => commands.removeOverlayBackground())
}
</script>

<template>
  <Sheet v-model:open="open">
    <SheetContent
      side="right"
      class="flex w-[420px] flex-col gap-0 p-0 shadow-[-12px_0_32px_rgb(0_0_0/0.45)] sm:max-w-[420px]"
      @open-auto-focus="focusChosenKind"
      @close-auto-focus="(event: Event) => emit('closeAutoFocus', event)"
    >
      <SheetHeader class="border-b border-border p-4">
        <SheetTitle>Event card</SheetTitle>
        <SheetDescription>Changes save as you make them.</SheetDescription>
      </SheetHeader>

      <div class="flex shrink-0 flex-col items-center gap-2 border-b border-border bg-muted/30 px-6 py-4">
        <EventPreviewControls v-model:kind="previewKind" v-model:sample="previewSample" fill />
        <div :style="{ width: `${previewBox.width}px`, height: `${previewBox.height}px` }">
          <div class="origin-top-left" :style="{ transform: scale < 1 ? `scale(${scale})` : undefined }">
            <LeagueOverlayCard :state="preview" :now="now" :readonly="true" />
          </div>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ scale < 1 ? 'Scaled to fit · sample data' : 'Sample data' }}
        </p>
      </div>

      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-6 pt-4 pb-6">
        <h3 class="text-xs font-medium tracking-wide text-muted-foreground uppercase">Look</h3>

        <div class="flex flex-col gap-2">
          <Label id="event-card-theme">Theme</Label>
          <SegmentedControl
            :model-value="theme"
            :options="THEME_OPTIONS"
            layout="fill"
            aria-labelledby="event-card-theme"
            @update:model-value="(value: string) => emit('change', { overlayLeagueTheme: value as LeagueTheme })"
          />
          <div
            v-if="usesClassicOptions(theme)"
            data-reveal="theme"
            class="flex flex-col gap-3 border-l-2 border-border pl-3"
          >
            <div class="flex flex-col gap-2">
              <Label id="event-card-artwork">Artwork</Label>
              <SegmentedControl
                :model-value="artwork"
                :options="ARTWORK_OPTIONS"
                layout="grid"
                aria-labelledby="event-card-artwork"
                @update:model-value="(value: string) => emit('change', { overlayArtwork: value as OverlayArtwork })"
              />
              <div
                v-if="artwork === 'custom'"
                data-reveal="artwork"
                class="flex flex-col gap-2 border-l-2 border-primary/60 pl-3"
              >
                <Label>Custom background</Label>
                <p class="text-xs text-muted-foreground">
                  Upload an image (for example channel art or a logo). It replaces the deck art everywhere. Recommended
                  900 × 300, at most 5 MB.
                </p>
                <div class="flex items-center gap-2">
                  <Button type="button" variant="outline" size="sm" :disabled="choosing" @click="chooseBackground">
                    {{ view.backgroundUrl ? 'Replace' : 'Upload' }}
                  </Button>
                  <Button
                    v-if="view.backgroundUrl"
                    type="button"
                    variant="ghost"
                    size="sm"
                    :disabled="choosing"
                    @click="removeBackground"
                  >
                    Remove
                  </Button>
                </div>
                <p v-if="failure" class="text-xs text-destructive">{{ failure }}</p>
              </div>
            </div>
            <div class="flex flex-col gap-2">
              <Label id="event-card-games">Games</Label>
              <SegmentedControl
                :model-value="display.games"
                :options="GAMES_OPTIONS"
                layout="fill"
                aria-labelledby="event-card-games"
                @update:model-value="(value: string) => setGames(value as LeagueGames)"
              />
            </div>
          </div>
        </div>

        <div class="flex flex-col gap-2">
          <Label id="event-card-size">Size</Label>
          <SegmentedControl
            :model-value="size"
            :options="SIZE_OPTIONS"
            layout="fill"
            aria-labelledby="event-card-size"
            @update:model-value="(value: string) => emit('change', { overlaySize: value as OverlaySize })"
          />
          <p class="text-xs text-muted-foreground">{{ sizeHint(theme, size, obs) }}</p>
        </div>

        <h3 class="mt-1 border-t pt-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">
          Show on the card
        </h3>
        <div class="flex flex-col gap-3">
          <div v-for="option in SHOW_OPTIONS" :key="option.key" class="flex items-center justify-between gap-4">
            <div class="min-w-0">
              <Label :for="`event-card-show-${option.key}`">
                {{ option.label }}
                <span
                  v-if="option.kind"
                  class="rounded-full border border-border bg-muted px-1.5 text-[11px] leading-4 font-medium text-muted-foreground"
                >
                  {{ KIND_TAGS[option.kind] }}
                </span>
              </Label>
              <p class="mt-0.5 text-xs text-muted-foreground">{{ option.example }}</p>
            </div>
            <Switch
              :id="`event-card-show-${option.key}`"
              :model-value="display[option.key]"
              @update:model-value="(on: boolean) => setShown(option, on)"
            />
          </div>
        </div>

        <h3 class="mt-1 border-t pt-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">
          Other event strip
        </h3>
        <div class="flex items-center justify-between gap-4">
          <div class="min-w-0">
            <Label for="event-card-strip">Other event strip</Label>
            <p class="mt-0.5 text-xs text-muted-foreground">{{ STRIP_SWITCH_HINT }}</p>
          </div>
          <Switch
            id="event-card-strip"
            :model-value="eventStrip"
            @update:model-value="(on: boolean) => emit('change', { overlayEventStrip: on })"
          />
        </div>

        <h3 class="mt-1 border-t pt-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">Timing</h3>
        <div class="flex flex-col gap-2">
          <Label for="event-card-finished-minutes">Finished event shown for</Label>
          <p class="text-xs text-muted-foreground">
            How long a finished or dropped event stays on the card and the stream overlay.
          </p>
          <div class="flex items-center gap-2">
            <Input
              id="event-card-finished-minutes"
              :model-value="finishedMinutes"
              type="number"
              inputmode="numeric"
              :min="MIN_FINISHED_MINUTES"
              :max="MAX_FINISHED_MINUTES"
              class="w-20"
              @change="setFinishedMinutes"
            />
            <span class="text-sm text-muted-foreground">minutes</span>
          </div>
        </div>
      </div>
    </SheetContent>
  </Sheet>
</template>
