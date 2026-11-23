<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import CoverCropDialog from '@/components/decks/CoverCropDialog.vue'
// The Cover Art card of the deck Settings page (0.x `pages/decks/Settings.vue`):
// pick a card name from the deck's latest version, then one of its printings
// with art; Remove clears the cover. The deck can also have an image of its
// own, kept on this device: it shows before the printing. An image larger
// than the cover size is cropped to the cover's shape first.
import { useQuery } from '@tanstack/vue-query'
import { computed, ref, watch } from 'vue'
import { commands, type CoverOptionView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Spinner } from '@/components/ui/spinner'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { COVER_HEIGHT, COVER_WIDTH, extensionOf, needsCrop } from '@/lib/coverCrop'
import { ARCHIVED_TITLE, coverAfterOptions, coverSettingsKey } from '@/lib/deckSettings'

const props = defineProps<{
  deckId: number
  archived: boolean
}>()

const { saving, run } = useDeckEdit()
const { add } = useToast()

const { data: coverSettings, error: coverError } = useQuery({
  queryKey: computed(() => coverSettingsKey(props.deckId)),
  queryFn: async ({ queryKey }) => unwrap(await commands.deckCoverSettings(queryKey[2])),
})

const currentCover = computed(() => coverSettings.value?.cover ?? null)
const cardNames = computed(() => coverSettings.value?.cardNames ?? [])
const customCover = computed(() => coverSettings.value?.customCoverArt ?? null)

const selectedCardName = ref('')
const artOptions = ref<CoverOptionView[]>([])
const selectedCoverId = ref<number | null>(null)
const loadingOptions = ref(false)
/** Only the latest name's printings are shown, however the loads finish. */
let latestRequest = 0

watch(selectedCardName, async (name) => {
  latestRequest += 1
  const request = latestRequest
  if (!name) {
    artOptions.value = []
    selectedCoverId.value = null
    loadingOptions.value = false
    return
  }

  loadingOptions.value = true
  try {
    const options = unwrap(await commands.deckCoverOptions(name))
    if (request !== latestRequest) {
      return
    }
    artOptions.value = options
    selectedCoverId.value = coverAfterOptions(options, selectedCoverId.value)
  } catch (error) {
    if (request === latestRequest) {
      add({ type: 'error', title: 'Could not load the art options', message: errorMessage(error) })
    }
  } finally {
    if (request === latestRequest) {
      loadingOptions.value = false
    }
  }
})

// Start from the saved cover, and follow it when it changes (a save, a
// remove, or a change made elsewhere). Declared after the name watcher, so
// a cover already cached at mount still loads its printings.
watch(
  () => currentCover.value?.catalogId ?? null,
  (catalogId) => {
    selectedCoverId.value = catalogId
    selectedCardName.value = currentCover.value?.name ?? ''
  },
  { immediate: true },
)

const hasChanged = computed(() => selectedCoverId.value !== (currentCover.value?.catalogId ?? null))
const selectedArt = computed(() => artOptions.value.find((option) => option.catalogId === selectedCoverId.value))

async function save() {
  const catalogId = selectedCoverId.value
  await run('Could not save the cover art', () => commands.setDeckCover(props.deckId, catalogId))
}

async function clear() {
  await run('Could not remove the cover art', () => commands.setDeckCover(props.deckId, null))
  // As 0.x: the picker empties whatever the outcome.
  selectedCardName.value = ''
  selectedCoverId.value = null
  artOptions.value = []
}

const fileInput = ref<HTMLInputElement | null>(null)
const cropDialog = ref<InstanceType<typeof CoverCropDialog> | null>(null)

function loadImage(file: File): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const image = new Image()
    image.onload = () => resolve(image)
    image.onerror = () => reject(new Error('The file could not be read as an image.'))
    image.src = URL.createObjectURL(file)
  })
}

async function store(bytes: Uint8Array, extension: string) {
  await run('Could not use that image', () => commands.setDeckCoverImage(props.deckId, Array.from(bytes), extension))
}

async function picked(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  let image: HTMLImageElement
  try {
    image = await loadImage(file)
  } catch (error) {
    add({ type: 'error', title: 'Could not use that image', message: errorMessage(error) })
    return
  }
  // A GIF keeps its animation, so it is stored as it is.
  if (file.type !== 'image/gif' && needsCrop({ width: image.naturalWidth, height: image.naturalHeight })) {
    cropDialog.value?.openFor(image, file.type)
    return
  }
  URL.revokeObjectURL(image.src)
  await store(new Uint8Array(await file.arrayBuffer()), extensionOf(file.name))
}

async function removeCustom() {
  await run('Could not remove the custom image', () => commands.removeDeckCover(props.deckId))
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>Cover Art</CardTitle>
      <CardDescription>Choose a card from your deck to use as cover art, or upload your own image.</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <p v-if="coverError" class="text-sm text-destructive">Could not load the deck's cards: {{ errorMessage(coverError) }}</p>

      <Select v-model="selectedCardName" :disabled="archived">
        <SelectTrigger class="w-full" :title="archived ? ARCHIVED_TITLE : undefined">
          <SelectValue placeholder="Select a card..." />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="name in cardNames" :key="name" :value="name">
            {{ name }}
          </SelectItem>
        </SelectContent>
      </Select>

      <div v-if="loadingOptions" class="flex items-center gap-2 text-sm text-muted-foreground">
        <Spinner class="size-4" />
        Loading art options...
      </div>

      <div
        v-if="artOptions.length > 1"
        class="flex flex-wrap gap-3"
        :class="archived ? 'pointer-events-none opacity-50' : ''"
        :title="archived ? ARCHIVED_TITLE : undefined"
      >
        <button
          v-for="option in artOptions"
          :key="option.catalogId"
          type="button"
          class="overflow-hidden rounded-md border-2 transition-all"
          :class="
            selectedCoverId === option.catalogId
              ? 'scale-105 border-primary ring-2 ring-primary/30'
              : 'border-border opacity-60 hover:border-muted-foreground hover:opacity-100'
          "
          :disabled="archived"
          @click="selectedCoverId = option.catalogId"
        >
          <CardImage :src="option.artCrop" :local="option.localArtCrop" :alt="selectedCardName" class="h-20 w-28 object-cover" />
        </button>
      </div>

      <div v-if="selectedArt" class="max-w-sm overflow-hidden rounded-lg border border-border">
        <CardImage :src="selectedArt.artCrop" :local="selectedArt.localArtCrop" :alt="selectedCardName" class="w-full object-cover" />
      </div>

      <div v-if="selectedCardName" class="flex items-center gap-2">
        <Button :disabled="!hasChanged || saving || archived" :title="archived ? ARCHIVED_TITLE : undefined" @click="save">
          <Spinner v-if="saving" class="mr-2 size-4" />
          Save
        </Button>
        <Button
          v-if="currentCover"
          variant="ghost"
          :disabled="saving || archived"
          :title="archived ? ARCHIVED_TITLE : undefined"
          @click="clear"
        >
          Remove
        </Button>
      </div>

      <div class="flex flex-col gap-3 border-t border-border pt-4">
        <div v-if="customCover" class="max-w-sm overflow-hidden rounded-lg border border-border">
          <CardImage :src="null" :custom="customCover" alt="Custom cover image" class="w-full object-cover" />
        </div>
        <p class="text-sm text-muted-foreground">
          Your own image shows on this device only. Other places, such as the hosted overlay, keep using the card above.
          Recommended size: {{ COVER_WIDTH }} × {{ COVER_HEIGHT }}. Larger images can be cropped.
        </p>
        <input ref="fileInput" type="file" accept="image/jpeg,image/png,image/webp,image/gif" class="hidden" @change="picked" />
        <CoverCropDialog ref="cropDialog" @crop="(image) => store(image.bytes, image.extension)" />
        <div class="flex items-center gap-2">
          <Button variant="outline" :disabled="saving || archived" :title="archived ? ARCHIVED_TITLE : undefined" @click="fileInput?.click()">
            Upload image
          </Button>
          <Button
            v-if="customCover"
            variant="ghost"
            :disabled="saving || archived"
            :title="archived ? ARCHIVED_TITLE : undefined"
            @click="removeCustom"
          >
            Remove custom image
          </Button>
        </div>
      </div>
    </CardContent>
  </Card>
</template>
