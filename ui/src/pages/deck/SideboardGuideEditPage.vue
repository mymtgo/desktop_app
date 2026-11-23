<script setup lang="ts">
// One sideboard guide (0.x `decks/SideboardGuideEdit`): the header with the
// deck's record against the archetype, the notes, the balance chip, and the
// "Bring in" and "Take out" columns over the deck's latest version. Every
// stepper click autosaves the whole plan after 500 ms. No community "Field"
// column (archetypes spec ruling 19).
import { useQueryClient } from '@tanstack/vue-query'
import { ArrowLeft } from 'lucide-vue-next'
import { computed, onBeforeUnmount, watch } from 'vue'
import { onBeforeRouteLeave, onBeforeRouteUpdate, useRoute } from 'vue-router'
import { commands, type GuideCardInputView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import SideboardGuideBalance from '@/components/guides/SideboardGuideBalance.vue'
import SideboardGuideCardRow from '@/components/guides/SideboardGuideCardRow.vue'
import SideboardGuideNotes from '@/components/guides/SideboardGuideNotes.vue'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckPage } from '@/composables/useDeckPage'
import { CommandFailed, errorMessage } from '@/lib/commands'
import {
  SAVE_FAILED,
  groupByType,
  guideKeys,
  guidesPath,
  postboardText,
  recordLine,
  seedSelection,
  selectionKey,
  selectionTotals,
  sidedInText,
  useGuideAutosave,
  useGuideEditor,
  youCutText,
} from '@/lib/guides'

const route = useRoute()
const queryClient = useQueryClient()
const { deckId } = useDeckPage()
const guideId = computed(() => Number(route.params.guideId) || 0)
const { data: page, error, isPending } = useGuideEditor(deckId, guideId)

/** One save: the first refused entry's message, or null once saved. */
async function save(cards: GuideCardInputView[]): Promise<string | null> {
  const result = await commands.saveGuideCards(deckId.value, guideId.value, cards)
  if (result.status === 'error') {
    return result.error.kind === 'refused' ? result.error.message : SAVE_FAILED
  }
  if (result.data.length > 0) return result.data[0].message
  // The list and the archetype page show the new plan; the editor keeps its selection.
  void queryClient.invalidateQueries({ queryKey: guideKeys.all })
  return null
}

const autosave = useGuideAutosave(save)
const { selection, state, error: saveError, label } = autosave

// Start from the saved plan when a guide opens; later refetches (after a
// save, a note or a deck sync) keep the selection being edited.
watch(
  () => (page.value ? page.value.guide.id : null),
  (id) => {
    if (id !== null && page.value) autosave.reset(seedSelection(page.value.sideboard))
  },
  { immediate: true },
)

// A waiting change goes out before the page (or its guide) changes.
onBeforeRouteLeave(() => autosave.leave())
onBeforeRouteUpdate(() => autosave.leave())
onBeforeUnmount(() => autosave.leave())

const totals = computed(() => selectionTotals(selection))
const groupedIn = computed(() => groupByType(page.value?.sideboard?.bringIn ?? []))
const groupedOut = computed(() => groupByType(page.value?.sideboard?.takeOut ?? []))
const bringInCount = computed(() => page.value?.sideboard?.bringIn.length ?? 0)

function planned(direction: 'in' | 'out', oracleId: string): number {
  return selection[selectionKey(direction, oracleId)] ?? 0
}

function loadError(failure: unknown): string {
  return failure instanceof CommandFailed ? failure.error.message : errorMessage(failure)
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <p v-if="error" class="text-sm text-destructive">Could not load the guide: {{ loadError(error) }}</p>

    <Skeleton v-else-if="isPending" class="h-40 w-full" />

    <div v-else-if="!page" class="flex flex-col items-center gap-2 py-16 text-center">
      <p class="text-sm font-medium">This guide no longer exists.</p>
      <RouterLink :to="guidesPath(deckId)" class="text-sm text-muted-foreground hover:underline">Back to guides</RouterLink>
    </div>

    <template v-else>
      <header class="flex flex-wrap items-center justify-between gap-3">
        <div class="flex items-center gap-3">
          <Button variant="ghost" size="icon" class="size-8" as-child>
            <RouterLink :to="guidesPath(deckId)" aria-label="Back to guides"><ArrowLeft class="size-4" /></RouterLink>
          </Button>
          <div class="flex flex-col gap-0.5">
            <h2 class="flex items-center gap-2 text-sm font-semibold">
              <ManaColors v-if="page.guide.colors.length" :colors="page.guide.colors" />
              vs {{ page.guide.archetypeName }}
            </h2>
            <p class="text-xs text-muted-foreground tabular-nums">{{ recordLine(page.guide) }}</p>
          </div>
        </div>
        <span
          v-if="label"
          class="text-xs tabular-nums"
          :class="state === 'error' ? 'text-red-300' : 'text-muted-foreground'"
          aria-live="polite"
        >
          {{ label }}
        </span>
      </header>

      <p v-if="saveError" class="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-300">
        {{ saveError }}
      </p>

      <SideboardGuideNotes
        :deck-id="deckId"
        :archetype-id="page.guide.archetypeId"
        :current="page.notes.current"
        :other="page.notes.other"
      />

      <h3 class="border-t border-border pt-4 text-sm font-semibold">Sideboarding</h3>

      <!-- Pinned over the lists, centred on the seam between them. -->
      <div v-if="page.hasVersion && page.sideboardSize > 0" class="sticky top-0 z-20 -mb-2 flex justify-center">
        <SideboardGuideBalance
          :total-in="totals.totalIn"
          :total-out="totals.totalOut"
          :sideboard-size="page.sideboardSize"
        />
      </div>

      <p
        v-if="!page.hasVersion || !page.sideboard"
        class="rounded-md border border-dashed border-border p-6 text-center text-sm text-muted-foreground"
      >
        This deck has no saved version yet, so there is nothing to plan with. Notes still work above.
      </p>

      <div v-else class="grid gap-4 lg:grid-cols-2">
        <section class="flex flex-col gap-2 rounded-md border border-border">
          <h3
            class="flex items-baseline justify-between border-b border-border px-3 py-2 text-[10px] font-semibold tracking-wider text-muted-foreground uppercase"
          >
            <span>Bring in</span>
            <span class="w-20 text-right">Your W–L</span>
          </h3>
          <p v-if="bringInCount === 0" class="px-3 pb-3 text-xs text-muted-foreground">
            No sideboard cards in the current list.
          </p>
          <div v-for="group in groupedIn" :key="group.type" class="px-3">
            <p class="py-1 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase">{{ group.type }}</p>
            <div class="divide-y divide-border/60">
              <SideboardGuideCardRow
                v-for="card in group.rows"
                :key="card.oracleId"
                :name="card.name"
                :image="card.image"
                :local-image="card.localImage"
                :owned="card.quantity"
                :planned="planned('in', card.oracleId)"
                :stale="card.stale"
                direction="in"
                @change="(value: number) => autosave.set('in', card.oracleId, value)"
              >
                <span class="w-20 shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                  {{ sidedInText(card) }}
                </span>
              </SideboardGuideCardRow>
            </div>
          </div>
        </section>

        <section class="flex flex-col gap-2 rounded-md border border-border">
          <h3
            class="flex items-baseline justify-between border-b border-border px-3 py-2 text-[10px] font-semibold tracking-wider text-muted-foreground uppercase"
          >
            <span>Take out</span>
            <span class="w-20 text-right">You cut</span>
          </h3>
          <div v-for="group in groupedOut" :key="group.type" class="px-3">
            <p class="py-1 text-[10px] font-semibold tracking-wider text-muted-foreground/60 uppercase">{{ group.type }}</p>
            <div class="divide-y divide-border/60">
              <SideboardGuideCardRow
                v-for="card in group.rows"
                :key="card.oracleId"
                :name="card.name"
                :image="card.image"
                :local-image="card.localImage"
                :owned="card.quantity"
                :planned="planned('out', card.oracleId)"
                :stale="card.stale"
                direction="out"
                @change="(value: number) => autosave.set('out', card.oracleId, value)"
              >
                <span class="w-20 shrink-0 text-right text-xs text-muted-foreground tabular-nums">
                  {{ youCutText(card) }}
                </span>
              </SideboardGuideCardRow>
            </div>
          </div>
        </section>
      </div>

      <p v-if="page.sideboard" class="text-[10px] tracking-wider text-muted-foreground/60 uppercase">
        {{ postboardText(page.sideboard.postboardGames, page.sideboard.postboardRecord) }}
      </p>
    </template>
  </div>
</template>
