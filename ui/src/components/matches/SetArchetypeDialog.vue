<script setup lang="ts">
import { computed, ref } from 'vue'
import ArchetypePicker from '@/components/matches/ArchetypePicker.vue'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useArchetypeOptions, useSetOpponentArchetype } from '@/composables/useOpponentArchetype'
import { setArchetypeDescription } from '@/lib/archetypePicker'

/**
 * "Set Archetype" (0.x `SetArchetypeDialog`): one match's opponent from the
 * match page or a row's menu (`openForMatch`), or the selected rows in bulk
 * (`openForMatches`). The picker offers the match format's archetypes and
 * the fallbacks; choosing one sets it by hand and closes the dialog.
 */
const emit = defineEmits<{ archetypeSet: [] }>()

const open = ref(false)
const tokens = ref<string[]>([])
const bulk = ref(false)
const format = ref<string | null>(null)
const currentId = ref<number | null>(null)

const { data: options } = useArchetypeOptions(format, open)
const { saving, setArchetype } = useSetOpponentArchetype()

const description = computed(() => setArchetypeDescription(bulk.value ? tokens.value.length : null))

function openForMatch(token: string, matchFormat: string | null, current: number | null = null) {
  tokens.value = [token]
  bulk.value = false
  format.value = matchFormat
  currentId.value = current
  open.value = true
}

function openForMatches(matchTokens: string[], matchFormat: string | null) {
  tokens.value = [...matchTokens]
  bulk.value = true
  format.value = matchFormat
  currentId.value = null
  open.value = true
}

async function select(archetypeId: number) {
  if (await setArchetype(tokens.value, archetypeId)) {
    open.value = false
    emit('archetypeSet')
  }
}

defineExpose({ openForMatch, openForMatches })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[80vh] flex-col">
      <DialogHeader>
        <DialogTitle>Set Archetype</DialogTitle>
        <DialogDescription>{{ description }}</DialogDescription>
      </DialogHeader>
      <ArchetypePicker
        v-if="open"
        :archetypes="options"
        :format="format"
        :current-id="currentId"
        show-fallbacks
        :disabled="saving"
        class="min-h-0 flex-1"
        list-class="max-h-none min-h-0 flex-1 border-0 p-0"
        autofocus
        @select="select"
      />
    </DialogContent>
  </Dialog>
</template>
