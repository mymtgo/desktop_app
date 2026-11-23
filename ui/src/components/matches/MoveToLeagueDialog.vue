<script setup lang="ts">
import { Check } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { RunStateView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useMatchEdit } from '@/composables/useMatchEdit'
import { startedDayMonth, statusLabel, whereNowText } from '@/lib/leagues'
import { useMatchLeagueTargets } from '@/lib/queries'

/**
 * "Move to league..." (league self-management spec 5.4): the leagues this
 * match may join (spec 4.1: same account and deck, a finished match, not
 * a tournament match), newest first, and "No league" (spec 4.2) for a
 * match in one. The store checks the rules again.
 */
const NO_LEAGUE = 'none'

const open = ref(false)
const token = ref<string | null>(null)
/** A run key, `NO_LEAGUE`, or `null` before a choice. */
const chosen = ref<string | null>(null)
const { saving, moveToLeague } = useMatchEdit()
const { data: view, isPending } = useMatchLeagueTargets(token)

const current = computed(() => view.value?.current ?? null)
const targets = computed(() => view.value?.targets ?? [])
const canSubmit = computed(() => !saving.value && chosen.value !== null)

function openForMatch(matchToken: string) {
  token.value = matchToken
  chosen.value = null
  open.value = true
}

function choose(choice: string) {
  chosen.value = choice
}

async function submit() {
  const matchToken = token.value
  const choice = chosen.value
  if (matchToken === null || choice === null || !canSubmit.value) return
  const runKey = choice === NO_LEAGUE ? null : choice
  if (await moveToLeague(matchToken, runKey, current.value)) open.value = false
}

defineExpose({ openForMatch })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-lg">
      <DialogHeader>
        <DialogTitle>Move to league</DialogTitle>
        <DialogDescription>Now {{ whereNowText(current) }}.</DialogDescription>
      </DialogHeader>

      <div class="max-h-[60vh] overflow-y-auto">
        <div v-if="isPending" class="space-y-2 p-2">
          <div class="h-10 animate-pulse rounded bg-muted" />
          <div class="h-10 animate-pulse rounded bg-muted" />
        </div>
        <ul v-else class="divide-y divide-border">
          <li v-for="target in targets" :key="target.key">
            <button
              type="button"
              :data-testid="`move-target-${target.key}`"
              class="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-muted/50"
              @click="choose(target.key)"
            >
              <Check class="size-4 shrink-0" :class="chosen === target.key ? '' : 'invisible'" />
              <div class="flex min-w-0 flex-1 flex-col">
                <span class="truncate text-sm font-medium">{{ target.name }}</span>
                <span class="truncate text-xs text-muted-foreground">
                  {{ startedDayMonth(target.startedAtMs) }} · {{ statusLabel(target.state as RunStateView) }} ·
                  {{ target.matchCount }} match{{ target.matchCount === 1 ? '' : 'es' }}
                </span>
              </div>
            </button>
          </li>
          <li v-if="current !== null">
            <button
              type="button"
              data-testid="move-target-none"
              class="flex w-full items-center gap-3 px-3 py-2 text-left hover:bg-muted/50"
              @click="choose(NO_LEAGUE)"
            >
              <Check class="size-4 shrink-0" :class="chosen === NO_LEAGUE ? '' : 'invisible'" />
              <span class="text-sm font-medium">No league</span>
            </button>
          </li>
        </ul>
        <p v-if="!isPending && targets.length === 0" class="p-6 text-center text-sm text-muted-foreground">
          No league can take this match: it needs the same deck and account, and tournament matches stay in their
          tournament.
        </p>
      </div>

      <DialogFooter>
        <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
        <Button type="button" data-testid="move-submit" :disabled="!canSubmit" @click="submit">Move</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
