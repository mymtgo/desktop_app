<script setup lang="ts">
// 0.x `DekUploadButton.vue`: the native file picker, then the cards resolved.
import { Upload } from 'lucide-vue-next'
import { ref } from 'vue'
import { commands, type ResolvedCardsView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { errorMessage, unwrap } from '@/lib/commands'

const emit = defineEmits<{ resolved: [data: ResolvedCardsView] }>()

const uploading = ref(false)
const error = ref<string | null>(null)
const loaded = ref(false)

async function pick() {
  uploading.value = true
  error.value = null
  try {
    const data = unwrap(await commands.readDekFile())
    if (data) {
      loaded.value = true
      emit('resolved', data)
    }
  } catch (failure) {
    error.value = errorMessage(failure) || 'Failed to parse deck file.'
    loaded.value = false
  } finally {
    uploading.value = false
  }
}
</script>

<template>
  <div>
    <Button type="button" variant="outline" class="w-full justify-start" :disabled="uploading" @click="pick">
      <Spinner v-if="uploading" class="mr-2 size-4" />
      <Upload v-else class="mr-2 size-4" />
      <span class="truncate">{{ uploading ? 'Parsing…' : loaded ? '.dek file loaded' : 'Upload .dek file' }}</span>
    </Button>
    <p v-if="error" class="mt-1.5 text-xs text-red-400">{{ error }}</p>
  </div>
</template>
