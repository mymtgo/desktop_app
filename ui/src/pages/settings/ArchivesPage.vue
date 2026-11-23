<script setup lang="ts">
import { computed, ref } from 'vue'
import ArchivedLeaguesTable from '@/components/archives/ArchivedLeaguesTable.vue'
import ArchivedMatchesTable from '@/components/archives/ArchivedMatchesTable.vue'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { archiveTabLabel } from '@/lib/archives'
import { useDeletedItems } from '@/lib/queries'

// Settings, Archives (league self-management spec 5.5): the matches and
// leagues the player deleted, a tab each, with Restore. Matches removed from
// stats before this release are here too. Nothing is purged.
// The tables wait for the first load, so an empty state never flashes.
const { data: deleted } = useDeletedItems()

const tab = ref<string>('matches')
const matches = computed(() => deleted.value?.matches ?? [])
const leagues = computed(() => deleted.value?.leagues ?? [])
</script>

<template>
  <Tabs v-model="tab" class="flex flex-col gap-4">
    <TabsList>
      <TabsTrigger value="matches">{{ archiveTabLabel('Matches', matches.length) }}</TabsTrigger>
      <TabsTrigger value="leagues">{{ archiveTabLabel('Leagues', leagues.length) }}</TabsTrigger>
    </TabsList>
    <TabsContent value="matches">
      <ArchivedMatchesTable v-if="deleted" :matches="matches" />
    </TabsContent>
    <TabsContent value="leagues">
      <ArchivedLeaguesTable v-if="deleted" :leagues="leagues" />
    </TabsContent>
  </Tabs>
</template>
