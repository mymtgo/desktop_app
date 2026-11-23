<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import LimitedSidebar from '@/components/limited/LimitedSidebar.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { usePageDetail } from '@/composables/useWindowTitle'
import { errorMessage } from '@/lib/commands'
import { useLimitedEvent } from '@/lib/queries'
import { useLimitedKey } from '@/pages/limited/useLimitedKey'

/**
 * A Limited event's pages (0.x `LimitedEventLayout`): the sidebar beside the
 * page. Each event mounts its pages afresh, so no page state carries across.
 */
const route = useRoute()
const key = useLimitedKey()
const { data: event, error } = useLimitedEvent(key)
usePageDetail(() => event.value?.title)

/** The sidebar entry of the page on screen (a match sits under Matches). */
const currentPage = computed(() => route.meta.limitedPage ?? '')
</script>

<template>
  <div v-if="event" class="flex min-h-0 flex-1 flex-col">
    <div class="flex min-h-0 flex-1">
      <div class="w-56 shrink-0">
        <LimitedSidebar :event="event" :current-page="currentPage" />
      </div>
      <div class="flex min-h-0 flex-1 flex-col overflow-y-auto border-l border-white/5">
        <RouterView :key="key" />
      </div>
    </div>
  </div>
  <div v-else-if="error" class="p-6 text-sm text-destructive">Could not load this event: {{ errorMessage(error) }}</div>
  <div v-else-if="event === null" class="flex flex-1 flex-col items-center justify-center gap-2 p-6 text-center">
    <p class="text-sm font-medium">This event no longer exists.</p>
    <RouterLink to="/limited" class="text-sm text-muted-foreground underline-offset-4 hover:underline">
      Back to Limited
    </RouterLink>
  </div>
  <div v-else class="flex min-h-0 flex-1">
    <div class="w-56 shrink-0 border-r border-black/80 p-3"><Skeleton class="h-24 w-full" /></div>
    <div class="flex-1 p-4"><Skeleton class="h-40 w-full" /></div>
  </div>
</template>
