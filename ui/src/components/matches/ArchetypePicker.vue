<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Check } from 'lucide-vue-next'
import { computed, nextTick, onMounted, ref, toRef } from 'vue'
import { type ArchetypeCatalogOption, type ArchetypePickerOption, commands } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { useArchetypeCatalog } from '@/composables/useOpponentArchetype'
import { useToast } from '@/composables/useToast'
import { type PickerRow, pickerRows, splitArchetypes } from '@/lib/archetypePicker'
import { errorMessage, unwrap } from '@/lib/commands'
import { cn } from '@/lib/utils'

/**
 * The one archetype search list (0.x `archetypes/ArchetypePicker`): the
 * match dialog, the manual match form and the deck pages render it inside
 * their own containers. Fallbacks (Homebrew, Rogue) come first with a
 * "System" badge when `showFallbacks`; the rest are the format's archetypes
 * whose name holds the search, the player's own with a "Custom" badge. `archetypes` undefined is still loading.
 * With a format it also lists the server's archetypes v1 has not met
 * (#489): picking one holds it first, then `select` has its new id.
 */
const props = withDefaults(
  defineProps<{
    archetypes: ArchetypePickerOption[] | undefined
    /** A format code or label; null keeps every format. */
    format?: string | null
    showFallbacks?: boolean
    disabled?: boolean
    autofocus?: boolean
    listClass?: string
    /** The archetype chosen now, marked with a tick. */
    currentId?: number | null
  }>(),
  { format: null, showFallbacks: false, disabled: false, autofocus: false, listClass: '', currentId: null },
)

const emit = defineEmits<{ select: [archetypeId: number] }>()

const search = ref('')
const searchInput = ref<{ $el: HTMLInputElement } | null>(null)

const split = computed(() => splitArchetypes(props.archetypes ?? [], props.format, search.value))
const fallbacks = computed(() => (props.showFallbacks ? split.value.fallbacks : []))

const { data: catalog } = useArchetypeCatalog(toRef(props, 'format'))
const rows = computed(() => pickerRows(split.value.regular, catalog.value, search.value))

const queryClient = useQueryClient()
const { add } = useToast()
const holding = ref(false)

onMounted(() => {
  if (props.autofocus) void nextTick(() => searchInput.value?.$el?.focus())
})

function choose(id: number) {
  emit('select', id)
  search.value = ''
}

function chooseRow(row: PickerRow) {
  if (row.id !== null) choose(row.id)
  else if (row.entry) void hold(row.entry)
}

/** A server archetype v1 has not met becomes a held one, then is chosen as any other. */
async function hold(entry: ArchetypeCatalogOption) {
  holding.value = true
  try {
    const id = unwrap(await commands.holdArchetype(entry.uuid, entry.name, entry.format, entry.colorIdentity))
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ['archetypeOptions'] }),
      queryClient.invalidateQueries({ queryKey: ['archetypeCatalog'] }),
    ])
    choose(id)
  } catch (error) {
    add({ type: 'error', title: 'Could not add the archetype', message: errorMessage(error) })
  } finally {
    holding.value = false
  }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <Input ref="searchInput" v-model="search" placeholder="Search archetypes..." :disabled="disabled || holding" />

    <div :class="cn('flex max-h-60 flex-col gap-0.5 overflow-y-auto rounded-md border border-border p-1', listClass)">
      <div v-if="archetypes === undefined" class="flex items-center justify-center py-6">
        <Spinner class="size-4" />
      </div>

      <template v-else>
        <template v-if="fallbacks.length">
          <Button
            v-for="archetype in fallbacks"
            :key="archetype.id"
            variant="ghost"
            class="w-full justify-between text-muted-foreground italic"
            :disabled="disabled || holding"
            @click="choose(archetype.id)"
          >
            <Check v-if="archetype.id === currentId" class="size-3.5 shrink-0" />
            <span class="flex-1 text-left">{{ archetype.name }}</span>
            <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wide uppercase">System</span>
          </Button>
          <div class="my-1 border-t border-border" />
        </template>

        <Button
          v-for="row in rows"
          :key="row.key"
          variant="ghost"
          class="w-full justify-between"
          :disabled="disabled || holding"
          @click="chooseRow(row)"
        >
          <Check v-if="row.id !== null && row.id === currentId" class="size-3.5 shrink-0" />
          <span class="flex-1 truncate text-left">{{ row.name }}</span>
          <span v-if="row.isCustom" class="rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wide uppercase">Custom</span>
          <ManaColors v-if="row.colors.length" :colors="row.colors" />
        </Button>

        <p
          v-if="fallbacks.length === 0 && rows.length === 0"
          class="py-4 text-center text-sm text-muted-foreground"
        >
          No archetypes found.
        </p>
      </template>
    </div>
  </div>
</template>
