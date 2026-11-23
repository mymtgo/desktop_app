<script setup lang="ts">
import { computed, onMounted, watch } from 'vue'
import { Separator } from '@/components/ui/separator'
import { Switch } from '@/components/ui/switch'
import { useLaunchAtLogin } from '@/composables/useLaunchAtLogin'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { closingWindowText } from '@/lib/settingsNav'
import { useAppInfo, useLegacyImportProgress, useSettings } from '@/lib/queries'

// 0.x `components/settings/BackgroundCard.vue`, with v1's close to tray
// switch inside "Closing the window" (spec ruling 3). Launch at login is the
// OS's own state (foundation spec 4.5).
const { data: info } = useAppInfo()
const { data: settings } = useSettings()
const { saving, save } = useSaveSettings()
const { enabled: launchAtLogin, changing: changingLaunch, load: loadLaunchAtLogin, set: setLaunchAtLogin } =
  useLaunchAtLogin()

onMounted(loadLaunchAtLogin)

// A 0.x import can turn launch at login on: read the OS state again once one
// finishes, and when the settings change (the refetch an import causes).
// Progress pushes in between do not reload it.
const { data: importProgress } = useLegacyImportProgress()
watch(
  () => importProgress.value?.phase === 'finished',
  (finished) => {
    if (finished) void loadLaunchAtLogin()
  },
)
watch(settings, () => void loadLaunchAtLogin())

const trayAvailable = computed(() => info.value?.trayAvailable === true)

function setCloseToTray(on: boolean) {
  if (settings.value) void save({ ...settings.value, closeToTray: on })
}
</script>

<template>
  <section class="space-y-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Background</h2>
      <p class="text-xs text-muted-foreground">
        Keep MyMTGO running in the system tray and launch automatically when you sign in.
      </p>
    </div>
    <div class="flex items-center justify-between gap-6">
      <div class="space-y-1">
        <p class="text-sm font-medium">Launch at login</p>
        <p class="text-xs text-muted-foreground">Start MyMTGO automatically when your computer signs in.</p>
      </div>
      <Switch :model-value="launchAtLogin" :disabled="changingLaunch" @update:model-value="setLaunchAtLogin" />
    </div>
    <Separator />
    <div class="space-y-1">
      <p class="text-sm font-medium">Closing the window</p>
      <p class="text-xs text-muted-foreground">{{ closingWindowText(trayAvailable, settings?.closeToTray !== false) }}</p>
    </div>
    <div v-if="trayAvailable && settings" class="flex items-center justify-between gap-6">
      <p class="text-sm font-medium">Close to tray</p>
      <Switch :model-value="settings.closeToTray" :disabled="saving" @update:model-value="setCloseToTray" />
    </div>
  </section>
</template>
