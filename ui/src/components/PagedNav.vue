<script setup lang="ts">
// The numbered page links every paged list shows: previous, the pages and
// next. Only the Matches page marks skipped pages with an ellipsis, so the
// others leave `showEllipsis` off. Classes go to the root, and the caller
// decides when it shows.
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationNext,
  PaginationPrevious,
} from '@/components/ui/pagination'

defineProps<{
  page: number
  perPage: number
  total: number
  showEllipsis?: boolean
}>()

const emit = defineEmits<{ 'update:page': [page: number] }>()
</script>

<template>
  <Pagination
    v-slot="{ page: current }"
    :items-per-page="perPage"
    :total="total"
    :page="page"
    @update:page="(next: number) => emit('update:page', next)"
  >
    <PaginationContent v-slot="{ items }">
      <PaginationPrevious />
      <template v-for="(item, index) in items" :key="index">
        <PaginationItem v-if="item.type === 'page'" :value="item.value" :is-active="item.value === current">
          {{ item.value }}
        </PaginationItem>
        <PaginationEllipsis v-else-if="showEllipsis" :index="index" />
      </template>
      <PaginationNext />
    </PaginationContent>
  </Pagination>
</template>
