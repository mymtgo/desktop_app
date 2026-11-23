<script setup lang="ts">
// The typical lists of an archetype (spec section 10), one at a time with
// a version picker (issue #326, as on the website): the live decklists, a
// manual archetype's variants, offline the stored key list; a fallback's
// 0.x text. The version shown is the route's (`version`).
import { computed } from 'vue'
import type { ArchetypeDecklistsView, ArchetypeSourceView, VariantView } from '@/bindings'
import VariantDecklist from '@/components/archetypes/VariantDecklist.vue'
import { Card } from '@/components/ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { decklistVersions, pickVersion } from '@/lib/archetypeDecklist'

const props = defineProps<{
  archetypeId: number
  archetypeName: string
  source: ArchetypeSourceView
  decklists: ArchetypeDecklistsView | undefined
  loading: boolean
  stored: VariantView[]
  /** The route's version key, `null` for the first. */
  version: string | null
}>()

const emit = defineEmits<{ changed: []; 'update:version': [string] }>()

const offered = computed(() => decklistVersions(props.source, props.decklists, props.loading, props.stored))
const shown = computed(() => pickVersion(offered.value.versions, props.version))
const chosenKey = computed({
  get: () => shown.value?.key ?? '',
  set: (key: string) => emit('update:version', key),
})
</script>

<template>
  <div class="flex flex-col gap-3">
    <!-- Without a list to show, the note is the card. -->
    <Card v-if="offered.note && !shown" class="p-4">
      <p class="text-sm text-muted-foreground">{{ offered.note }}</p>
    </Card>
    <p v-else-if="offered.note" class="text-sm text-muted-foreground">{{ offered.note }}</p>
    <VariantDecklist
      v-if="shown"
      :key="shown.key"
      :archetype-id="archetypeId"
      :archetype-name="archetypeName"
      :list="shown.list"
      :variant-label="shown.label"
      :cards="shown.cards"
      :editable="shown.editable"
      :can-remove="offered.versions.length > 1"
      :live="shown.live"
      layout="text"
      @changed="emit('changed')"
    >
      <template #picker>
        <Select v-if="offered.versions.length > 1" v-model="chosenKey">
          <SelectTrigger class="w-64" aria-label="Decklist version">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="version in offered.versions" :key="version.key" :value="version.key">
              {{ version.label }}
            </SelectItem>
          </SelectContent>
        </Select>
        <h3 v-else class="text-sm font-semibold">{{ shown.label }}</h3>
      </template>
    </VariantDecklist>
  </div>
</template>
