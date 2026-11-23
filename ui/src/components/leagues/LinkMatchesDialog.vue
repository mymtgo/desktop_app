<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Check } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { commands } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useLeagueEdit } from '@/composables/useLeagueEdit'
import { useToast } from '@/composables/useToast'
import { lastPlayedText } from '@/lib/decks'
import { keys } from '@/lib/keys'
import { movedFromText, NO_VALUE, remainingSlots, whereNowText } from '@/lib/leagues'
import { useLeagueAvailableMatches } from '@/lib/queries'

/**
 * "Add matches..." (0.x `AddMatchDialog`, league self-management spec
 * 5.3): the matches any listed league may take (spec 4.1), each with
 * where it sits now. A manual league takes up to its free slots; a log or
 * imported run has no cap. The toast names where a single match came from.
 */
const queryClient = useQueryClient()
const open = ref(false)
const runKey = ref<string | null>(null)
const { add } = useToast()
/** The most that can be picked; `null` for no cap. */
const slots = ref<number | null>(5)
/** A limited run takes matches of its set, not of its deck. */
const limited = ref(false)
const selected = ref<Set<string>>(new Set())
const { saving, run } = useLeagueEdit()

const { data: matches, isPending, dataUpdatedAt } = useLeagueAvailableMatches(runKey)

/** What the listed matches have in common with the run. */
const played = computed(() => (limited.value ? 'of this set' : 'played with this deck'))
const selectedCount = computed(() => selected.value.size)
const canSubmit = computed(
  () => !saving.value && selectedCount.value > 0 && (slots.value === null || selectedCount.value <= slots.value),
)

function full(): boolean {
  return slots.value !== null && selected.value.size >= slots.value
}

function openForRun(key: string, currentMatchCount: number, manual: boolean, limitedRun: boolean) {
  runKey.value = key
  limited.value = limitedRun
  slots.value = manual ? remainingSlots(currentMatchCount) : null
  selected.value = new Set()
  open.value = true
  // A match may have finished since the list was last read.
  void queryClient.invalidateQueries({ queryKey: keys.leagueAvailableMatches(key) })
}

function toggle(token: string) {
  const next = new Set(selected.value)
  if (next.has(token)) {
    next.delete(token)
  } else if (!full()) {
    next.add(token)
  } else {
    return
  }
  selected.value = next
}

async function submit() {
  const key = runKey.value
  if (key === null || !canSubmit.value) return
  const tokens = Array.from(selected.value)
  const done = await run('Could not add matches', () => commands.linkLeagueMatch(key, tokens))
  if (!done) return
  open.value = false
  if (tokens.length === 1) {
    const from = matches.value?.find((match) => match.token === tokens[0])?.currentLeague ?? null
    add({ type: 'success', title: 'Match added', message: movedFromText(from) })
  } else {
    add({ type: 'success', title: 'Matches added', message: `${tokens.length} matches were added.` })
  }
}

function resultTone(result: string | null): string {
  if (result === 'W') return 'text-success'
  if (result === 'L') return 'text-destructive'
  return 'text-muted-foreground'
}

defineExpose({ openForRun })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Add matches</DialogTitle>
        <DialogDescription>
          <template v-if="slots !== null">
            Select up to {{ slots }} match{{ slots === 1 ? '' : 'es' }} {{ played }}.
          </template>
          <template v-else>Select matches {{ played }}.</template>
        </DialogDescription>
      </DialogHeader>

      <div class="max-h-[60vh] overflow-y-auto">
        <div v-if="isPending" class="space-y-2 p-2">
          <div class="h-10 animate-pulse rounded bg-muted" />
          <div class="h-10 animate-pulse rounded bg-muted" />
          <div class="h-10 animate-pulse rounded bg-muted" />
        </div>

        <div v-else-if="!matches?.length" class="p-6 text-center text-sm text-muted-foreground">
          No matches this league can take.
        </div>

        <ul v-else class="divide-y divide-border">
          <li v-for="match in matches" :key="match.token">
            <button
              type="button"
              :data-token="match.token"
              class="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-muted/50 disabled:opacity-50 disabled:hover:bg-transparent"
              :disabled="saving || (!selected.has(match.token) && full())"
              @click="toggle(match.token)"
            >
              <div
                class="flex size-5 shrink-0 items-center justify-center rounded border"
                :class="selected.has(match.token) ? 'border-primary bg-primary text-primary-foreground' : 'border-border bg-card'"
              >
                <Check v-if="selected.has(match.token)" class="size-3.5" />
              </div>
              <div class="flex min-w-0 flex-1 flex-col">
                <span class="truncate text-sm font-medium">{{ match.opponentName ?? 'Unknown opponent' }}</span>
                <span class="truncate text-xs text-muted-foreground">
                  {{ match.opponentArchetype ?? 'Unknown archetype' }} ·
                  {{ lastPlayedText(match.startedAtMs, dataUpdatedAt) }} · {{ whereNowText(match.currentLeague) }}
                </span>
              </div>
              <div class="flex shrink-0 items-center gap-2 text-xs tabular-nums">
                <span :class="resultTone(match.result)">{{ match.result ?? NO_VALUE }}</span>
                <span class="text-muted-foreground">{{ match.gameRecord }}</span>
              </div>
            </button>
          </li>
        </ul>
      </div>

      <DialogFooter class="flex items-center justify-between gap-2 sm:justify-between">
        <p class="text-xs text-muted-foreground">
          {{ slots !== null ? `${selectedCount} of ${slots} selected` : `${selectedCount} selected` }}
        </p>
        <div class="flex gap-2">
          <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
          <Button type="button" data-testid="link-matches-submit" :disabled="!canSubmit" @click="submit">
            Add {{ selectedCount > 0 ? selectedCount : '' }} match{{ selectedCount === 1 ? '' : 'es' }}
          </Button>
        </div>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
