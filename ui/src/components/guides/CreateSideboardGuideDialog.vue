<script setup lang="ts">
// Pick an archetype to start a guide for (0.x `CreateSideboardGuideDialog`).
// A pair that already has a guide is refused; the message shows inline and
// the dialog stays open so another archetype can be chosen. The archetypes
// load once the list has painted (0.x deferred them).
import { useQueryClient } from '@tanstack/vue-query'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { commands } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { splitArchetypes } from '@/lib/archetypePicker'
import { guideKeys, guidePath, useCreateGuide, useGuideArchetypeOptions } from '@/lib/guides'

const props = defineProps<{
  deckId: number
  /** The deck's format code (`CMODERN`). */
  format: string
}>()

const router = useRouter()
const queryClient = useQueryClient()

const dialog = useCreateGuide((archetypeId) => commands.createGuide(props.deckId, archetypeId))
const { open, search, error, processing } = dialog

const loaded = ref(false)
const format = computed<string | null>(() => props.format)
const { data: archetypes } = useGuideArchetypeOptions(format, loaded)

const split = computed(() => splitArchetypes(archetypes.value ?? [], props.format, search.value))

/** Ask for the archetypes after the list painted. */
function load() {
  loaded.value = true
}

function show() {
  load()
  dialog.show()
}

async function select(archetypeId: number) {
  const guideId = await dialog.select(archetypeId)
  if (guideId === null) return
  await queryClient.invalidateQueries({ queryKey: guideKeys.all })
  await router.push(guidePath(props.deckId, guideId))
}

defineExpose({ show, load })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[80vh] flex-col">
      <DialogHeader>
        <DialogTitle>Create sideboard guide</DialogTitle>
        <DialogDescription>Choose the opponent archetype this guide is for.</DialogDescription>
      </DialogHeader>

      <Input v-model="search" placeholder="Search archetypes..." />

      <p v-if="error" class="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-300">
        {{ error }}
      </p>

      <div class="flex-1 space-y-0.5 overflow-y-auto">
        <template v-if="split.fallbacks.length">
          <Button
            v-for="archetype in split.fallbacks"
            :key="archetype.id"
            variant="ghost"
            class="w-full justify-between text-muted-foreground italic"
            :disabled="processing"
            @click="select(archetype.id)"
          >
            <span class="flex-1 text-left">{{ archetype.name }}</span>
            <span class="rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wide uppercase">System</span>
          </Button>
          <div class="my-1 border-t border-border" />
        </template>

        <Button
          v-for="archetype in split.regular"
          :key="archetype.id"
          variant="ghost"
          class="w-full justify-between"
          :disabled="processing"
          @click="select(archetype.id)"
        >
          <span class="flex-1 text-left">{{ archetype.name }}</span>
          <ManaColors v-if="archetype.colors.length" :colors="archetype.colors" />
        </Button>

        <p
          v-if="split.fallbacks.length === 0 && split.regular.length === 0"
          class="py-4 text-center text-sm text-muted-foreground"
        >
          <template v-if="!archetypes">Loading archetypes…</template>
          <template v-else>No archetypes found.</template>
        </p>
      </div>
    </DialogContent>
  </Dialog>
</template>
