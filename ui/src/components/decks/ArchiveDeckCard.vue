<script setup lang="ts">
// The danger zone of the deck Settings page (0.x `pages/decks/Settings.vue`):
// type DELETE to archive the deck (0.x deleted it); an archived deck shows
// Restore instead.
import { TriangleAlert, Undo2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { commands } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { ARCHIVE_KEYWORD, canArchive } from '@/lib/deckSettings'

const props = defineProps<{
  deckId: number
  archived: boolean
}>()

const router = useRouter()
const { saving: archiving, run: runArchive } = useDeckEdit()
const { saving: restoring, run: runRestore } = useDeckEdit()
const confirmation = ref('')

const allowed = computed(() => canArchive(confirmation.value))

async function archiveDeck() {
  if (!allowed.value || archiving.value) {
    return
  }
  const done = await runArchive('Could not archive the deck', () =>
    commands.archiveDeck(props.deckId, confirmation.value),
  )
  if (done) {
    // 0.x went back to the deck list after a delete.
    void router.push({ name: 'decks' })
  }
}

async function restoreDeck() {
  await runRestore('Could not restore the deck', () => commands.restoreDeck(props.deckId))
  confirmation.value = ''
}
</script>

<template>
  <Card class="border-destructive/40">
    <CardHeader>
      <CardTitle class="flex items-center gap-2 text-destructive">
        <TriangleAlert class="size-4" />
        Danger zone
      </CardTitle>
      <CardDescription>
        Archiving a deck hides it from your deck list and makes it read-only. Match history is kept, so you can restore
        the deck at any time.
      </CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <template v-if="archived">
        <p class="text-sm text-muted-foreground">This deck is archived and read-only. Restore it to make changes again.</p>
        <div>
          <Button variant="outline" :disabled="restoring" @click="restoreDeck">
            <Spinner v-if="restoring" class="mr-2 size-4" />
            <Undo2 v-else class="mr-2 size-4" />
            Restore deck
          </Button>
        </div>
      </template>
      <template v-else>
        <div class="flex flex-col gap-2">
          <label for="archive-confirmation" class="text-sm text-muted-foreground">
            Type <span class="font-mono font-medium text-foreground">{{ ARCHIVE_KEYWORD }}</span> to confirm.
          </label>
          <div class="flex flex-col gap-2 sm:flex-row">
            <Input
              id="archive-confirmation"
              :model-value="confirmation"
              autocomplete="off"
              :placeholder="ARCHIVE_KEYWORD"
              class="sm:flex-1"
              :disabled="archiving"
              @update:model-value="(value) => (confirmation = String(value))"
              @keydown.enter.prevent="archiveDeck"
            />
            <Button variant="destructive" :disabled="!allowed || archiving" @click="archiveDeck">
              <Spinner v-if="archiving" class="mr-2 size-4" />
              Archive deck
            </Button>
          </div>
        </div>
      </template>
    </CardContent>
  </Card>
</template>
