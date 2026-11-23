<script setup lang="ts">
import { Tags, X } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import ArchetypePicker from '@/components/matches/ArchetypePicker.vue'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useArchetypeOptions } from '@/composables/useOpponentArchetype'

/**
 * The deck index's bulk bar (0.x `DeckSelectionBar`): "Assign archetype"
 * with the picker scoped to the selection's format, "Clear archetype", and
 * "Deselect".
 */
const props = defineProps<{
  count: number
  busy: boolean
  /** The selected decks' shared format, or null when they span formats. */
  format: string | null
}>()

const emit = defineEmits<{
  assign: [archetypeId: number | null]
  dismiss: []
}>()

const pickerOpen = ref(false)
const { data: options } = useArchetypeOptions(
  computed(() => props.format),
  computed(() => pickerOpen.value),
)

function onPick(archetypeId: number) {
  pickerOpen.value = false
  emit('assign', archetypeId)
}
</script>

<template>
  <div v-if="count > 0" class="flex items-center gap-3 border-t border-black/60 bg-muted/50 px-4 py-2">
    <span class="text-sm font-medium tabular-nums">{{ count }} selected</span>

    <Popover v-model:open="pickerOpen">
      <PopoverTrigger as-child>
        <Button variant="outline" size="sm" class="gap-1.5" :disabled="busy">
          <Tags class="size-3.5" />
          Assign archetype
        </Button>
      </PopoverTrigger>
      <PopoverContent side="top" align="start" class="w-72 p-2">
        <ArchetypePicker :archetypes="options" :format="format" autofocus @select="onPick" />
      </PopoverContent>
    </Popover>

    <Button variant="ghost" size="sm" class="gap-1.5 text-muted-foreground" :disabled="busy" @click="emit('assign', null)">
      Clear archetype
    </Button>

    <Button variant="ghost" size="sm" class="ml-auto gap-1.5 text-muted-foreground" @click="emit('dismiss')">
      <X class="size-3.5" />
      Deselect
    </Button>
  </div>
</template>
