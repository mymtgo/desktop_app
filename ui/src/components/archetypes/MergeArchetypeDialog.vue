<script setup lang="ts">
// 0.x `MergeArchetypeDialog.vue`. Also used by the archetype detail (T10).
import { useQuery } from '@tanstack/vue-query'
import { computed, ref, watch } from 'vue'
import { commands, type ArchetypeChoiceView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useToast } from '@/composables/useToast'
import { archetypeKeys, candidateLoader, filterCandidates } from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'

const props = defineProps<{ archetype: { id: number; name: string } }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ merged: [] }>()

const { add } = useToast()
const selected = ref<ArchetypeChoiceView | null>(null)
const search = ref('')
const submitting = ref(false)

const { data, isPending } = useQuery({
  queryKey: computed(() => archetypeKeys.candidates('merge', props.archetype.id)),
  queryFn: ({ queryKey }) => candidateLoader('merge')(queryKey[3]),
  enabled: open,
})

watch(open, (next) => {
  if (!next) {
    selected.value = null
    search.value = ''
  }
})

const filtered = computed(() => filterCandidates(data.value ?? [], search.value))

async function submit() {
  if (selected.value === null) return
  submitting.value = true
  try {
    const message = unwrap(await commands.mergeArchetype(props.archetype.id, selected.value.id))
    add({ type: 'success', title: message, message: '' })
    emit('merged')
    open.value = false
  } catch (failure) {
    add({ type: 'error', title: 'Could not merge', message: errorMessage(failure) })
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-w-md">
      <DialogHeader>
        <DialogTitle>Merge {{ archetype.name }}</DialogTitle>
        <DialogDescription>
          Pick the archetype to merge this one into. Future detections of {{ archetype.name }} will be attributed to the
          parent. Existing matches are not changed.
        </DialogDescription>
      </DialogHeader>

      <Input v-model="search" placeholder="Search archetypes" class="mb-2" />

      <div class="max-h-72 overflow-y-auto rounded border">
        <p v-if="isPending" class="p-3 text-sm text-muted-foreground">Loading…</p>
        <p v-else-if="filtered.length === 0" class="p-3 text-sm text-muted-foreground">No candidates available.</p>
        <button
          v-for="candidate in filtered"
          :key="candidate.id"
          type="button"
          class="flex w-full items-center justify-between gap-3 border-b px-3 py-2 text-left text-sm last:border-b-0 hover:bg-muted"
          :class="{ 'bg-muted': selected?.id === candidate.id }"
          @click="selected = candidate"
        >
          <span>{{ candidate.name }}</span>
          <span class="text-xs text-muted-foreground uppercase">{{ candidate.format }}</span>
        </button>
      </div>

      <p v-if="selected" class="mt-2 text-sm text-muted-foreground">
        Future detections of <strong>{{ archetype.name }}</strong> will be attributed to
        <strong>{{ selected.name }}</strong>. You can unmerge later.
      </p>

      <DialogFooter>
        <Button variant="outline" @click="open = false">Cancel</Button>
        <Button :disabled="selected === null || submitting" @click="submit">Merge</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
