<script setup lang="ts">
import { useIntervalFn } from '@vueuse/core'
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import type { LeagueTheme, ObsSizes, OverlayPublishChanged, OverlaySize, Settings } from '@/bindings'
import {
  OBS_REFRESH_TIP,
  copySizeLabel,
  copySizeText,
  lastPublishedText,
  obsUrl as obsUrlOf,
  publishErrorText,
  publishHint,
  stripResizeNote,
} from '@/lib/overlaySettings'

// Where the event card shows (event overlay spec 4.2): the desktop window
// and the hosted OBS overlay (0.x StreamOverlayCard). The publish state
// arrives as `overlay-publish-changed` pushes (no poll); the "last
// published" time ticks every 5 s. On native Wayland only the window switch
// is unavailable; publishing works on every platform.
const props = defineProps<{
  /** `leagueWindow`. */
  windowEnabled: boolean
  windowDisabled: boolean
  /** Why overlay windows are unavailable, if they are. */
  note: string | null
  /** `overlayPublish`. */
  enabled: boolean
  url: string | null
  size: OverlaySize
  theme: LeagueTheme
  /**
   * The Browser Source sizes for the saved theme and strip switch (Rust's,
   * `OverlaySettingsView.obsSizes`); `null` while a change to them is saving.
   */
  obsSizes: ObsSizes | null
  /** `overlayEventStrip`: the sizes include the strip. */
  strip: boolean
  publish: OverlayPublishChanged
  signedIn: boolean
  disabled: boolean
}>()

const emit = defineEmits<{ change: [change: Partial<Settings>] }>()

const copied = ref<'url' | 'size' | null>(null)
const now = ref(Date.now())
useIntervalFn(() => (now.value = Date.now()), 5000)

const obsUrl = computed(() => obsUrlOf(props.url, props.size))
const obsSize = computed(() => props.obsSizes?.[props.size] ?? null)
const lastPublished = computed(() => lastPublishedText(props.publish.lastPublishedAtMs, now.value))
const errorMessage = computed(() => publishErrorText(props.publish.error))

async function copy(what: 'url' | 'size') {
  const text = what === 'url' ? obsUrl.value : obsSize.value && copySizeText(obsSize.value)
  if (!text) return
  await navigator.clipboard.writeText(text)
  copied.value = what
  setTimeout(() => (copied.value = null), 1500)
}
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Where it shows</h2>
      <p class="text-xs text-muted-foreground">Both use the event card above.</p>
    </div>
    <p v-if="note" class="text-sm text-warning">{{ note }}</p>
    <div class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <Label for="event-output-window">Desktop window</Label>
        <p class="text-sm text-muted-foreground">A small always-on-top window. Opens while an event is active.</p>
      </div>
      <Switch
        id="event-output-window"
        :model-value="windowEnabled"
        :disabled="windowDisabled"
        @update:model-value="(on: boolean) => emit('change', { leagueWindow: on })"
      />
    </div>
    <div class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <Label for="event-output-obs">Stream overlay (OBS)</Label>
        <p class="text-sm text-muted-foreground">
          Add it to OBS as a Browser Source. Works with the desktop window closed. {{ publishHint(signedIn) }}
        </p>
      </div>
      <Switch
        id="event-output-obs"
        :model-value="enabled"
        :disabled="disabled"
        @update:model-value="(on: boolean) => emit('change', { overlayPublish: on })"
      />
    </div>

    <div v-if="enabled && url" class="flex flex-col gap-2">
      <div class="flex items-center gap-2">
        <code class="min-w-0 flex-1 truncate rounded bg-muted px-2 py-1.5 text-xs">{{ obsUrl }}</code>
        <Button size="sm" variant="outline" class="cursor-pointer" @click="copy('url')">
          {{ copied === 'url' ? 'Copied' : 'Copy' }}
        </Button>
      </div>
      <div class="flex items-center justify-between gap-2">
        <p v-if="obsSize" class="text-xs text-muted-foreground">
          In OBS, set this Browser Source to
          <span class="font-medium text-foreground">{{ obsSize.width }} × {{ obsSize.height }}</span> (width ×
          height).
        </p>
        <p v-else class="text-xs text-muted-foreground">In OBS, set this Browser Source to the new size (updating).</p>
        <Button size="sm" variant="outline" class="shrink-0 cursor-pointer" :disabled="!obsSize" @click="copy('size')">
          {{ obsSize ? copySizeLabel(obsSize, copied === 'size') : 'Copy size' }}
        </Button>
      </div>
      <p v-if="strip" class="text-xs text-muted-foreground">{{ stripResizeNote(theme, size) }}</p>
      <p class="text-xs text-muted-foreground">Changing the theme changes this size: update the Browser Source in OBS.</p>
      <p class="text-xs text-muted-foreground">{{ OBS_REFRESH_TIP }}</p>
      <p class="text-xs text-muted-foreground">{{ lastPublished }}.</p>
      <p v-if="errorMessage" class="text-xs text-destructive">{{ errorMessage }}</p>
    </div>
    <p v-else-if="enabled && !url" class="text-xs text-muted-foreground">Sign in to MTGO to get your overlay URL.</p>
  </section>
</template>
