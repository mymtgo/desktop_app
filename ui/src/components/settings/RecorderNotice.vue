<script setup lang="ts">
/**
 * The recorder's risk notice (spec 2026-11-09 section 5). Nothing downloads
 * before "Download and enable" is pressed; it stays disabled until the box is
 * ticked. The box starts unticked every time the notice opens.
 */
import { ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Label } from '@/components/ui/label'
import { commands } from '@/bindings'
import { recorderFileUrl } from '@/lib/recorder'

const props = defineProps<{ open: boolean; version: string; submitting?: boolean; error?: string | null }>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  accept: []
}>()

const accepted = ref(false)
watch(
  () => props.open,
  () => {
    accepted.value = false
  },
)

const EULA_URL = 'https://www.mtgo.com/eula'
const SDK_URL = 'https://github.com/videre-project/MTGOSDK'
const SDK_LICENCE_URL = 'https://github.com/videre-project/MTGOSDK/blob/main/LICENSE'

function noticeUrl(file: string): string {
  return recorderFileUrl(props.version, file)
}

function open(url: string) {
  void commands.openExternal(url)
}
</script>

<template>
  <Dialog :open="props.open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-h-[90vh] max-w-2xl overflow-y-auto">
      <DialogHeader>
        <DialogTitle>Record MTGO replays</DialogTitle>
        <DialogDescription>Read this first. Nothing downloads until you press the button at the bottom.</DialogDescription>
      </DialogHeader>

      <div class="flex flex-col gap-4 text-sm">
        <section class="space-y-1">
          <h3 class="font-semibold">What it does</h3>
          <ul class="list-disc space-y-1 pl-5 text-muted-foreground">
            <li>
              While you play, it reads game information from your running MTGO client using MTGOSDK, an unofficial library
              that loads code into the MTGO process in order to read it.
            </li>
            <li>
              It writes one replay file per match to <code>Documents\MtgReplay\</code>, and MyMTGO adds them to your
              matches. The files contain your MTGO username and your opponents'. If your Documents folder syncs to OneDrive
              or similar, they sync too.
            </li>
            <li>
              On test (edge) versions of MyMTGO, it also keeps debug files for each match, so problems can be fixed: a log
              beside the replays and raw captures in <code>%LOCALAPPDATA%\MtgoReplay\captures\</code>. They hold the same
              usernames and are not cleaned up. Stable versions never keep these.
            </li>
          </ul>
        </section>

        <section class="space-y-1">
          <h3 class="font-semibold">What it doesn't do</h3>
          <ul class="list-disc space-y-1 pl-5 text-muted-foreground">
            <li>It never plays for you, makes moves, or changes the game.</li>
            <li>It records only what your own client shows you; your opponent's hidden cards stay hidden.</li>
            <li>
              MyMTGO itself still only reads log files. Turning this off stops the recorder MyMTGO runs.
            </li>
          </ul>
        </section>

        <section class="space-y-1">
          <h3 class="font-semibold">Why we think it is fine, and why we cannot promise it</h3>
          <ul class="list-disc space-y-1 pl-5 text-muted-foreground">
            <li>It only reads, and shows you nothing your own screen did not.</li>
            <li>As far as we know, Daybreak has not acted against read-only recorders.</li>
            <li>
              Daybreak's MTGO EULA (section 14) lets Daybreak suspend or close accounts that run programs collecting game
              information, without notice, including access to your collection. Daybreak has not approved this tool. If you
              have a second account, consider using it.
            </li>
            <li>
              Because it loads code into MTGO, it could make MTGO slow, freeze or crash, including mid-match.
            </li>
          </ul>
        </section>

        <p>
          <strong>You are downloading and running this at your own risk.</strong>
          MyMTGO works fully without it. The recorder is not code signed, so Windows or your antivirus may warn about it.
        </p>

        <div class="flex flex-wrap gap-x-4 gap-y-1">
          <Button variant="link" size="sm" class="h-auto p-0" @click="open(EULA_URL)">Daybreak MTGO EULA</Button>
          <Button variant="link" size="sm" class="h-auto p-0" @click="open(SDK_URL)">MTGOSDK</Button>
          <Button variant="link" size="sm" class="h-auto p-0" @click="open(SDK_LICENCE_URL)">MTGOSDK licence</Button>
          <Button variant="link" size="sm" class="h-auto p-0" @click="open(noticeUrl('NOTICE'))">Recorder notice</Button>
          <Button variant="link" size="sm" class="h-auto p-0" @click="open(noticeUrl('THIRD-PARTY-NOTICES.md'))">
            Third-party notices
          </Button>
        </div>

        <div class="flex items-center gap-2">
          <Checkbox id="recorder-accept" v-model="accepted" />
          <Label for="recorder-accept">I understand and accept the risk</Label>
        </div>
      </div>

      <p v-if="props.error" class="text-sm text-destructive" data-testid="recorder-notice-error">{{ props.error }}</p>

      <DialogFooter>
        <Button variant="outline" :disabled="props.submitting" @click="emit('update:open', false)">Cancel</Button>
        <Button :disabled="!accepted || props.submitting" @click="emit('accept')">Download and enable</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
