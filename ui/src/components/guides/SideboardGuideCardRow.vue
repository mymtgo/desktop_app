<script setup lang="ts">
// One card of the guide editor (0.x `SideboardGuideCardRow`): a stepper for
// the planned copies up to the zone's copies, the card and its name, then the
// numbers the caller slots in. Hovering the card or its name previews it. A stale row (the card left the zone this row
// plans against) can only go down, and shows a badge instead of the ceiling.
import { Minus, Plus } from 'lucide-vue-next'
import CardImage from '@/components/CardImage.vue'
import CardHoverPreview from '@/components/cards/CardHoverPreview.vue'
import { Button } from '@/components/ui/button'
import { canStepUp, staleLabel, stepPlanned, type Direction } from '@/lib/guides'

const props = defineProps<{
  name: string
  image: string | null
  localImage: string | null
  owned: number
  planned: number
  stale: boolean
  direction: Direction
}>()

const emit = defineEmits<{ change: [value: number] }>()

function step(delta: number) {
  const next = stepPlanned(props.planned, props.owned, props.stale, delta)
  if (next !== props.planned) emit('change', next)
}
</script>

<template>
  <div class="flex items-center gap-2 py-1.5 text-sm" :class="{ 'opacity-60': planned === 0 && !stale }">
    <div class="flex shrink-0 items-center rounded-md border border-border">
      <Button
        variant="ghost"
        size="icon"
        class="size-7 rounded-r-none"
        :disabled="planned === 0"
        aria-label="Fewer"
        @click="step(-1)"
      >
        <Minus class="size-3" />
      </Button>
      <span
        class="w-7 text-center font-semibold tabular-nums"
        :class="planned > 0 ? 'text-foreground' : 'text-muted-foreground'"
      >
        {{ planned }}
      </span>
      <Button
        variant="ghost"
        size="icon"
        class="size-7 rounded-l-none"
        :disabled="!canStepUp(planned, owned, stale)"
        aria-label="More"
        @click="step(1)"
      >
        <Plus class="size-3" />
      </Button>
    </div>
    <!-- Fixed width so a stale row, with no ceiling to show, keeps the names aligned. -->
    <span class="w-6 shrink-0 text-xs text-muted-foreground tabular-nums">
      <template v-if="!stale">/ {{ owned }}</template>
    </span>
    <!-- Only the card and its name preview the card: hovering the stepper must not cover the other column. -->
    <CardHoverPreview :image="image" :local-image="localImage" :name="name">
      <span class="flex min-w-0 grow items-center gap-2">
        <span class="h-7 w-5 shrink-0 overflow-hidden rounded bg-black/20">
          <CardImage :src="image" :local="localImage" :alt="name" class="h-full w-full object-cover object-top" />
        </span>
        <span class="min-w-0 truncate">
          {{ name }}
          <span
            v-if="stale"
            class="ml-1 rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-semibold tracking-wide text-amber-300 uppercase"
          >
            {{ staleLabel(direction) }}
          </span>
        </span>
      </span>
    </CardHoverPreview>
    <slot />
  </div>
</template>
