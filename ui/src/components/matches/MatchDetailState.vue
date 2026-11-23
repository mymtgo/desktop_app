<script setup lang="ts">
// A match page's body in each state: the detail, the load error, the
// skeleton while loading, and "This match no longer exists" once it is gone.
import type { MatchDetail } from '@/bindings'
import MatchDetailBody from '@/components/matches/MatchDetailBody.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/commands'

withDefaults(
  defineProps<{
    /** `null` once the match is gone. */
    match: MatchDetail | null | undefined
    error: unknown
    pending: boolean
    /** Where "Back to matches" goes when there is no history. */
    fallbackPath: string
    detectable?: boolean
    /** Detection is running on this match. */
    detecting?: boolean
  }>(),
  { detectable: false, detecting: false },
)

const emit = defineEmits<{ detect: [] }>()
</script>

<template>
  <MatchDetailBody
    v-if="match"
    :match="match"
    :fallback-path="fallbackPath"
    :detectable="detectable"
    :detecting="detecting"
    @detect="emit('detect')"
  />
  <div v-else-if="error" class="p-6 text-sm text-destructive">Could not load this match: {{ errorMessage(error) }}</div>
  <div v-else-if="pending" class="flex flex-col gap-5 p-4 lg:p-6">
    <Skeleton class="h-5 w-32" />
    <Skeleton class="h-16 w-full" />
    <Skeleton class="h-64 w-full" />
  </div>
  <div v-else class="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
    <p class="text-sm font-medium">This match no longer exists.</p>
    <RouterLink :to="fallbackPath" class="text-sm text-muted-foreground underline-offset-4 hover:underline">
      Back to matches
    </RouterLink>
  </div>
</template>
