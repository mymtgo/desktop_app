<script setup lang="ts">
// Crops a deck's own cover image to the cover's shape before it is stored:
// drag to move the image, the slider to zoom. The crop is drawn at the cover
// size (smaller when the crop is) and handed back as bytes.
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import {
  COVER_HEIGHT,
  COVER_WIDTH,
  type Offset,
  type Size,
  centredOffset,
  clampOffset,
  coverScale,
  outputSize,
  outputType,
  sourceRect,
} from '@/lib/coverCrop'

const emit = defineEmits<{ crop: [image: { bytes: Uint8Array; extension: string }] }>()

const FRAME: Size = { width: 560, height: Math.round((560 * COVER_HEIGHT) / COVER_WIDTH) }
const MAX_ZOOM = 4

const open = ref(false)
const source = ref<HTMLImageElement | null>(null)
const mime = ref('')
const zoom = ref(1)
const offset = ref<Offset>({ x: 0, y: 0 })
const cropping = ref(false)
let drag: { pointer: Offset; offset: Offset } | null = null

const imageSize = computed<Size>(() => ({
  width: source.value?.naturalWidth ?? 1,
  height: source.value?.naturalHeight ?? 1,
}))
const scale = computed(() => coverScale(imageSize.value, FRAME) * zoom.value)

function openFor(image: HTMLImageElement, type: string) {
  source.value = image
  mime.value = type
  zoom.value = 1
  offset.value = centredOffset(imageSize.value, scale.value, FRAME)
  open.value = true
}

function setZoom(next: number) {
  const before = scale.value
  zoom.value = next
  const ratio = scale.value / before
  const centre = { x: FRAME.width / 2, y: FRAME.height / 2 }
  offset.value = clampOffset(
    {
      x: centre.x - (centre.x - offset.value.x) * ratio,
      y: centre.y - (centre.y - offset.value.y) * ratio,
    },
    imageSize.value,
    scale.value,
    FRAME,
  )
}

function startDrag(event: PointerEvent) {
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  drag = { pointer: { x: event.clientX, y: event.clientY }, offset: offset.value }
}

function moveDrag(event: PointerEvent) {
  if (!drag) return
  offset.value = clampOffset(
    {
      x: drag.offset.x + event.clientX - drag.pointer.x,
      y: drag.offset.y + event.clientY - drag.pointer.y,
    },
    imageSize.value,
    scale.value,
    FRAME,
  )
}

function endDrag() {
  drag = null
}

async function crop() {
  const image = source.value
  if (!image) return
  cropping.value = true
  try {
    const rect = sourceRect(offset.value, scale.value, FRAME)
    const size = outputSize(rect)
    const canvas = document.createElement('canvas')
    canvas.width = size.width
    canvas.height = size.height
    canvas.getContext('2d')?.drawImage(image, rect.x, rect.y, rect.width, rect.height, 0, 0, size.width, size.height)
    const type = outputType(mime.value)
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, type.mime, 0.9))
    if (!blob) return
    emit('crop', { bytes: new Uint8Array(await blob.arrayBuffer()), extension: type.extension })
    open.value = false
  } finally {
    cropping.value = false
  }
}

defineExpose({ openFor })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-[608px]">
      <DialogHeader>
        <DialogTitle>Crop cover image</DialogTitle>
        <DialogDescription>
          Drag to move the image and use the slider to zoom. The cover is stored at {{ COVER_WIDTH }} × {{ COVER_HEIGHT }}.
        </DialogDescription>
      </DialogHeader>
      <div
        class="relative cursor-grab touch-none overflow-hidden rounded-md border border-border bg-black active:cursor-grabbing"
        :style="{ width: `${FRAME.width}px`, height: `${FRAME.height}px` }"
        @pointerdown="startDrag"
        @pointermove="moveDrag"
        @pointerup="endDrag"
        @pointercancel="endDrag"
      >
        <img
          v-if="source"
          :src="source.src"
          alt="Cover image to crop"
          draggable="false"
          class="pointer-events-none absolute top-0 left-0 max-w-none origin-top-left select-none"
          :style="{
            width: `${imageSize.width}px`,
            height: `${imageSize.height}px`,
            transform: `translate(${offset.x}px, ${offset.y}px) scale(${scale})`,
          }"
        />
      </div>
      <label class="flex items-center gap-3 text-sm text-muted-foreground">
        Zoom
        <input
          type="range"
          class="flex-1 accent-primary"
          min="1"
          :max="MAX_ZOOM"
          step="0.01"
          :value="zoom"
          @input="setZoom(Number(($event.target as HTMLInputElement).value))"
        />
      </label>
      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button size="sm" :disabled="cropping" @click="crop">Use image</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
