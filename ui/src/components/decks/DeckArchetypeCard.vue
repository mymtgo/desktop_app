<script setup lang="ts">
import { computed, ref } from 'vue'
import { commands, type DeckArchetypeView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import ArchetypePicker from '@/components/matches/ArchetypePicker.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { useArchetypeOptions } from '@/composables/useOpponentArchetype'
import { COMMANDER_ARCHETYPE_NOTE } from '@/lib/decklist'
import { ARCHIVED_TITLE } from '@/lib/deckSettings'

/**
 * The deck Settings page's "Archetype" card (0.x `decks/Settings`): the
 * archetype with "Change" and "Remove", or "Set Archetype"; the picker
 * offers the deck format's archetypes. An archived deck is read-only.
 */
const props = defineProps<{
  deckId: number
  /** The deck's format code. */
  format: string
  /** An Arena Brawl deck (the backend's `commanderFormat`). */
  commanderFormat: boolean
  archetype: DeckArchetypeView | null
  archived: boolean
}>()

const open = ref(false)
// A Brawl deck takes its commander's archetype, so the picker is read-only.
const locked = computed(() => props.commanderFormat)
const { data: options } = useArchetypeOptions(
  computed(() => props.format),
  computed(() => open.value),
)
const { saving, run } = useDeckEdit()

async function choose(archetypeId: number | null) {
  const done = await run('Could not set the archetype', () => commands.updateDeckArchetype(props.deckId, archetypeId))
  if (done) open.value = false
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>Archetype</CardTitle>
      <CardDescription>
        {{ locked ? COMMANDER_ARCHETYPE_NOTE : 'Set the archetype for this deck. This is used when reporting matches.' }}
      </CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <div v-if="archetype" class="flex items-center justify-between rounded-md border border-border bg-muted/30 px-4 py-3">
        <div class="flex items-center gap-3">
          <span class="font-medium">{{ archetype.name }}</span>
          <span v-if="archetype.isCustom" class="rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wide uppercase">Custom</span>
          <ManaColors v-if="archetype.colors.length" :colors="archetype.colors" />
        </div>
        <div v-if="!locked" class="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            :disabled="archived"
            :title="archived ? ARCHIVED_TITLE : undefined"
            @click="open = !open"
          >
            Change
          </Button>
          <Button
            variant="ghost"
            size="sm"
            :disabled="saving || archived"
            :title="archived ? ARCHIVED_TITLE : undefined"
            @click="choose(null)"
          >
            Remove
          </Button>
        </div>
      </div>
      <p v-else-if="locked" class="text-sm text-muted-foreground">No commander archetype yet. It is set when a match with this deck is read.</p>
      <div v-else>
        <Button variant="outline" :disabled="archived" :title="archived ? ARCHIVED_TITLE : undefined" @click="open = !open">
          {{ open ? 'Cancel' : 'Set Archetype' }}
        </Button>
      </div>

      <ArchetypePicker
        v-if="open && !locked"
        :archetypes="options"
        :format="format"
        :current-id="archetype?.id ?? null"
        :disabled="saving || archived"
        autofocus
        @select="choose"
      />
    </CardContent>
  </Card>
</template>
