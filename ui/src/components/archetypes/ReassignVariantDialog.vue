<script setup lang="ts">
// 0.x `ReassignVariantDialog.vue`.
import { useQuery } from '@tanstack/vue-query'
import { computed, ref, watch } from 'vue'
import { commands, type ArchetypeChoiceView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { useToast } from '@/composables/useToast'
import { archetypeKeys, candidateLoader, filterCandidates } from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'

const props = defineProps<{ archetypeId: number; archetypeName: string; list: number; variantLabel: string }>()
const open = defineModel<boolean>('open', { default: false })
const emit = defineEmits<{ reassigned: [] }>()

const { add } = useToast()
const selected = ref<ArchetypeChoiceView | null>(null)
const search = ref('')
const submitting = ref(false)

const { data, isPending } = useQuery({
  queryKey: computed(() => archetypeKeys.candidates('reassign', props.archetypeId)),
  queryFn: ({ queryKey }) => candidateLoader('reassign')(queryKey[3]),
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
    const message = unwrap(await commands.reassignVariant(props.archetypeId, props.list, selected.value.id))
    add({ type: 'success', title: message, message: '' })
    emit('reassigned')
    open.value = false
  } catch (failure) {
    add({ type: 'error', title: 'Could not reassign', message: errorMessage(failure) })
  } finally {
    submitting.value = false
  }
}
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="max-w-md">
      <DialogHeader>
        <DialogTitle>Reassign {{ variantLabel }}</DialogTitle>
        <DialogDescription>
          Move this variant to a different archetype. If this leaves {{ archetypeName }} with no variants it will be
          flagged as merged into the chosen archetype.
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
        {{ variantLabel }} will be moved to <strong>{{ selected.name }}</strong>.
      </p>

      <DialogFooter>
        <Button variant="outline" @click="open = false">Cancel</Button>
        <Button :disabled="selected === null || submitting" @click="submit">Reassign</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
