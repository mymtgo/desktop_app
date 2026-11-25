<script setup lang="ts">
// "Copy as image" (spec section 9): the target as a PNG on the clipboard,
// or "Save image" when the clipboard refuses (WebKitGTK).
import { Copy, Download, Loader2 } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { useScreenshot } from '@/composables/useScreenshot'
import { trackUsage } from '@/lib/usage'

const props = defineProps<{ target: HTMLElement | null; fileName: string }>()
// After a copy starts, for a page that counts what was copied.
const emit = defineEmits<{ copied: [] }>()

const { capture, capturing, save, saving, saveOffered } = useScreenshot()

function copy() {
  if (!props.target) return
  void capture(props.target, props.fileName)
  trackUsage({ kind: 'featureUsed', feature: 'copyImage' })
  emit('copied')
}
</script>

<template>
  <div class="flex items-center gap-2">
    <Button size="sm" variant="outline" :disabled="capturing || !target" @click="copy">
      <Loader2 v-if="capturing" class="mr-1.5 size-3.5 animate-spin" />
      <Copy v-else class="mr-1.5 size-3.5" />
      Copy as image
    </Button>
    <Button v-if="saveOffered" size="sm" variant="outline" :disabled="saving" @click="save">
      <Download class="mr-1.5 size-3.5" />
      Save image
    </Button>
  </div>
</template>
