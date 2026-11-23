<script setup lang="ts">
import { BookOpen } from 'lucide-vue-next'
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import type { LimitedIndexQuery } from '@/bindings'
import LimitedEventRow from '@/components/limited/LimitedEventRow.vue'
import LimitedFilters from '@/components/limited/LimitedFilters.vue'
import LimitedKpis from '@/components/limited/LimitedKpis.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { errorMessage } from '@/lib/commands'
import { limitedQueryFromUrl, limitedUrlQuery } from '@/lib/limited'
import { useLimitedIndex } from '@/lib/queries'

// The filters live in the URL (0.x kept them in the query string too), so
// back and forward step through them.
const route = useRoute()
const router = useRouter()

// Arena runs take only a status: the backend refuses their other edits.
const { platform } = usePlatformContext()

const query = computed(() => limitedQueryFromUrl(route.query))

const { data: index, error, isPending } = useLimitedIndex(query)

function setQuery(next: LimitedIndexQuery) {
  void router.replace({ query: limitedUrlQuery(next) })
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex items-center justify-between gap-2">
      <h1 class="text-base font-semibold tracking-tight">Limited</h1>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load Limited: {{ errorMessage(error) }}</p>

    <template v-else-if="isPending || !index">
      <div class="grid grid-cols-2 gap-4 md:grid-cols-3 lg:grid-cols-6">
        <Skeleton v-for="n in 6" :key="n" class="h-24 w-full" />
      </div>
      <Skeleton v-for="n in 4" :key="n" class="h-16 w-full" />
    </template>

    <template v-else>
      <LimitedKpis :kpis="index.kpis" />

      <LimitedFilters :filters="query" :sets="index.sets" @change="setQuery" />

      <div v-if="!index.rows.length" class="flex flex-col items-center gap-2 py-16 text-center">
        <BookOpen class="size-10 text-muted-foreground/40" />
        <p class="font-medium">No limited events yet.</p>
        <p class="text-sm text-muted-foreground">Join a draft league and mymtgo will record every pick.</p>
      </div>

      <div v-else class="flex flex-col gap-2">
        <LimitedEventRow
          v-for="(row, position) in index.rows"
          :key="row.runKey ?? `draft-${row.draftToken}`"
          :row="row"
          :default-expanded="position === 0"
          :editable="platform === 'mtgo'"
          :platform="platform"
        />
      </div>
    </template>
  </div>
</template>
