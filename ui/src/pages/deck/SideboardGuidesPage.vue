<script setup lang="ts">
// A deck's Sideboard Guides tab (0.x `decks/SideboardGuides`,
// `DeckSideboardGuides`): one row per guide with its plan, the deck's record
// against the archetype and its notes; create and delete. Archived decks
// included.
import { useQueryClient } from '@tanstack/vue-query'
import { BookOpen, Plus, Trash2 } from 'lucide-vue-next'
import { ref, watch } from 'vue'
import { commands, type GuideSummaryView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import CreateSideboardGuideDialog from '@/components/guides/CreateSideboardGuideDialog.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Empty, EmptyDescription } from '@/components/ui/empty'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useCurrentDeckHeader, useDeckPage } from '@/composables/useDeckPage'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { deleteGuideText, guideKeys, guidePath, percentCell, planSummary, recordCell, useDeckGuides } from '@/lib/guides'

const { deckId } = useDeckPage()
const { data: header } = useCurrentDeckHeader(useDeckBestOfContext().bestOf)
const { data: guides, error, isPending } = useDeckGuides(deckId)
const queryClient = useQueryClient()
const { add } = useToast()

const createDialog = ref<InstanceType<typeof CreateSideboardGuideDialog> | null>(null)

// The dialog's archetypes load once the list has painted (0.x deferred them).
watch([guides, createDialog], ([value, dialog]) => {
  if (value && dialog) dialog.load()
})

const pendingDelete = ref<GuideSummaryView | null>(null)
const deleting = ref(false)

async function confirmDelete() {
  const guide = pendingDelete.value
  if (!guide || deleting.value) return
  deleting.value = true
  try {
    unwrap(await commands.deleteGuide(guide.deckId, guide.id))
    await queryClient.invalidateQueries({ queryKey: guideKeys.all })
  } catch (failure) {
    add({ type: 'error', title: 'Could not delete the guide', message: errorMessage(failure) })
  } finally {
    deleting.value = false
    pendingDelete.value = null
  }
}
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <section class="flex flex-col gap-3">
      <header class="flex items-center justify-between gap-3">
        <div class="flex flex-col gap-0.5">
          <h2 class="text-sm font-semibold">Sideboard guides</h2>
          <p class="text-xs text-muted-foreground">
            Your own plan and notes for each matchup, shown in the game overlay when you face it.
          </p>
        </div>
        <Button size="sm" :disabled="!header" @click="createDialog?.show()">
          <Plus class="size-4" />
          Create guide
        </Button>
      </header>

      <p v-if="error" class="text-sm text-destructive">Could not load the guides: {{ errorMessage(error) }}</p>

      <Skeleton v-else-if="isPending" class="h-32 w-full" />

      <Empty v-else-if="!guides || guides.length === 0" class="border border-dashed border-border py-10">
        <BookOpen class="size-6 text-muted-foreground" />
        <EmptyDescription>No sideboard guides yet. Create one for an archetype you keep running into.</EmptyDescription>
        <Button size="sm" variant="outline" :disabled="!header" @click="createDialog?.show()">Create guide</Button>
      </Empty>

      <div v-else class="overflow-hidden rounded-md border border-border">
        <table class="w-full text-sm">
          <thead class="bg-muted/30 text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">
            <tr>
              <th class="px-3 py-2 text-left">Archetype</th>
              <th class="px-3 py-2 text-left">Plan</th>
              <th class="px-3 py-2 text-right">Matches</th>
              <th class="px-3 py-2 text-right">Match %</th>
              <th class="px-3 py-2 text-right">Game %</th>
              <th class="px-3 py-2 text-right">Notes</th>
              <th class="px-3 py-2 text-right">Updated</th>
              <th class="w-10 px-2 py-2"></th>
            </tr>
          </thead>
          <tbody class="divide-y divide-border">
            <tr v-for="guide in guides" :key="guide.id" class="group hover:bg-muted/40">
              <td class="px-3 py-2">
                <RouterLink
                  :to="guidePath(guide.deckId, guide.id)"
                  class="flex items-center gap-2 font-medium hover:underline"
                >
                  <ManaColors v-if="guide.colors.length" :colors="guide.colors" />
                  {{ guide.archetypeName }}
                </RouterLink>
              </td>
              <td class="px-3 py-2 text-muted-foreground tabular-nums">{{ planSummary(guide.cardsIn, guide.cardsOut) }}</td>
              <td class="px-3 py-2 text-right tabular-nums" :class="{ 'text-muted-foreground': guide.matches === 0 }">
                {{ recordCell(guide.matches, guide.matchRecord) }}
              </td>
              <td class="px-3 py-2 text-right tabular-nums" :class="{ 'text-muted-foreground': guide.matchWinrate === null }">
                {{ percentCell(guide.matchWinrate) }}
              </td>
              <td class="px-3 py-2 text-right tabular-nums" :class="{ 'text-muted-foreground': guide.gameWinrate === null }">
                {{ percentCell(guide.gameWinrate) }}
              </td>
              <td class="px-3 py-2 text-right tabular-nums">{{ guide.notesCount }}</td>
              <td class="px-3 py-2 text-right text-muted-foreground tabular-nums">{{ guide.updatedLabel }}</td>
              <td class="px-2 py-2 text-right">
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
                  :aria-label="`Delete guide for ${guide.archetypeName}`"
                  @click="pendingDelete = guide"
                >
                  <Trash2 class="size-3.5" />
                </Button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>

      <CreateSideboardGuideDialog v-if="header" ref="createDialog" :deck-id="deckId" :format="header.deck.format" />

      <Dialog :open="pendingDelete !== null" @update:open="(open: boolean) => !open && (pendingDelete = null)">
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Delete this guide?</DialogTitle>
            <DialogDescription>{{ deleteGuideText(pendingDelete?.archetypeName ?? '') }}</DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" :disabled="deleting" @click="pendingDelete = null">Cancel</Button>
            <Button variant="destructive" :disabled="deleting" @click="confirmDelete">Delete</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  </div>
</template>
