<script setup lang="ts">
// The archetype page's "Sideboard guides and notes" block (archetypes spec
// section 21.4): one card per deck with a guide or notes against the
// archetype, then "Add note" and "Create guide" for one of the player's decks
// of the archetype's format (the deck most played against it first). A
// retired archetype's block is read-only apart from deleting notes.
import { useQueryClient } from '@tanstack/vue-query'
import { Trash2 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { commands } from '@/bindings'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import {
  NO_DECKS_TEXT,
  NO_GUIDES_TEXT,
  defaultDeck,
  guideKeys,
  guidePath,
  percentCell,
  planSummary,
  recordCell,
  useArchetypeGuides,
  useCreateGuide,
  useGuideDeckOptions,
} from '@/lib/guides'

const props = defineProps<{
  archetypeId: number
  retired: boolean
}>()

const router = useRouter()
const queryClient = useQueryClient()
const { add } = useToast()

const archetypeId = computed(() => props.archetypeId)
const { data: rows, isPending } = useArchetypeGuides(archetypeId)
const { data: options } = useGuideDeckOptions(archetypeId)

/** The chosen deck, as the select's text value. */
const chosenDeck = ref('')
watch(
  () => options.value,
  (value) => {
    if (!value) return
    const kept = value.some((option) => String(option.deckId) === chosenDeck.value)
    if (!kept) {
      const deck = defaultDeck(value)
      chosenDeck.value = deck === null ? '' : String(deck)
    }
  },
  { immediate: true },
)
const deckId = computed(() => Number(chosenDeck.value) || 0)
const canWrite = computed(() => !props.retired && (options.value?.length ?? 0) > 0)

async function refresh() {
  await queryClient.invalidateQueries({ queryKey: guideKeys.all })
}

const body = ref('')
const savingNote = ref(false)

async function addNote() {
  if (!body.value.trim() || savingNote.value || deckId.value === 0) return
  savingNote.value = true
  try {
    unwrap(await commands.addNote(deckId.value, props.archetypeId, body.value))
    body.value = ''
    await refresh()
  } catch (failure) {
    add({ type: 'error', title: 'Could not add the note', message: errorMessage(failure) })
  } finally {
    savingNote.value = false
  }
}

async function removeNote(id: number) {
  try {
    unwrap(await commands.deleteNote(id))
    await refresh()
  } catch (failure) {
    add({ type: 'error', title: 'Could not delete the note', message: errorMessage(failure) })
  }
}

// "Create guide" refuses a deck that already has one, with 0.x's message
// kept beside the button.
const creator = useCreateGuide((archetypeId) => commands.createGuide(deckId.value, archetypeId))
const { error: createError, processing: creating } = creator

async function createGuide() {
  if (deckId.value === 0) return
  const created = deckId.value
  const guideId = await creator.select(props.archetypeId)
  if (guideId === null) return
  await refresh()
  await router.push(guidePath(created, guideId))
}

watch(chosenDeck, () => {
  createError.value = null
})
</script>

<template>
  <Card>
    <CardHeader><CardTitle class="text-sm">Sideboard guides and notes</CardTitle></CardHeader>
    <CardContent class="flex flex-col gap-4">
      <Skeleton v-if="isPending" class="h-24 w-full" />
      <p v-else-if="!rows || rows.length === 0" class="text-sm text-muted-foreground">{{ NO_GUIDES_TEXT }}</p>

      <div v-else class="grid grid-cols-1 gap-3 xl:grid-cols-2">
        <article v-for="row in rows" :key="row.deckId" class="flex flex-col gap-2 rounded-md border border-border p-3">
          <header class="flex flex-wrap items-center justify-between gap-2">
            <div class="flex min-w-0 items-center gap-2">
              <RouterLink :to="`/decks/${row.deckId}`" class="truncate font-medium hover:underline">{{ row.deckName }}</RouterLink>
              <Badge v-if="row.deckArchived" variant="outline">Archived</Badge>
            </div>
            <Button v-if="row.guideId !== null" size="sm" variant="outline" as-child>
              <RouterLink :to="guidePath(row.deckId, row.guideId)">Open guide</RouterLink>
            </Button>
          </header>
          <p class="text-xs text-muted-foreground tabular-nums">
            {{ planSummary(row.cardsIn, row.cardsOut) }} ·
            {{ recordCell(row.matches, row.matchRecord) }}
            <template v-if="row.matches > 0">
              · {{ percentCell(row.matchWinrate) }} match · {{ percentCell(row.gameWinrate) }} game
            </template>
          </p>
          <div v-if="row.plannedIn.length || row.plannedOut.length" class="grid grid-cols-2 gap-2 text-xs">
            <div>
              <p class="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">In</p>
              <p v-for="card in row.plannedIn" :key="card.oracleId" class="truncate tabular-nums">
                {{ card.quantity }} {{ card.name }}
              </p>
            </div>
            <div>
              <p class="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">Out</p>
              <p v-for="card in row.plannedOut" :key="card.oracleId" class="truncate tabular-nums">
                {{ card.quantity }} {{ card.name }}
              </p>
            </div>
          </div>
          <article
            v-for="note in row.notes"
            :key="note.id"
            class="flex items-start justify-between gap-2 rounded-md border border-border/60 px-3 py-2"
          >
            <div class="flex min-w-0 flex-col gap-1">
              <p class="text-sm whitespace-pre-wrap">{{ note.body }}</p>
              <time class="text-[10px] text-muted-foreground tabular-nums">{{ note.createdLabel }}</time>
            </div>
            <Button variant="ghost" size="icon" class="size-6 shrink-0" aria-label="Delete note" @click="removeNote(note.id)">
              <Trash2 class="size-3" />
            </Button>
          </article>
        </article>
      </div>

      <template v-if="!retired && options">
        <p v-if="options.length === 0" class="text-xs text-muted-foreground">{{ NO_DECKS_TEXT }}</p>
        <div v-else class="flex flex-col gap-2 border-t border-border pt-4">
          <Select v-model="chosenDeck">
            <SelectTrigger class="w-64" aria-label="Deck">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="option in options" :key="option.deckId" :value="String(option.deckId)">
                {{ option.name }}
              </SelectItem>
            </SelectContent>
          </Select>
          <textarea
            v-model="body"
            rows="3"
            maxlength="2000"
            placeholder="What did you learn this time?"
            class="w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-sm"
          />
          <p v-if="createError" class="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-300">
            {{ createError }}
          </p>
          <div class="flex justify-end gap-2">
            <Button size="sm" variant="outline" :disabled="!canWrite || creating" @click="createGuide">Create guide</Button>
            <Button size="sm" :disabled="!canWrite || savingNote || !body.trim()" @click="addNote">Add note</Button>
          </div>
        </div>
      </template>
    </CardContent>
  </Card>
</template>
