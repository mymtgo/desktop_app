<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import type { MostPickedView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { ALL_SETS, limitedPickName, pickSetChoices } from '@/lib/homeDashboard'
import { useDashboardPicks } from '@/lib/queries'

/**
 * Most picked (dashboard redesign spec 4.8): the six cards picked most in
 * the newest drafted set, another set, or every set. The dashboard brings
 * the newest set's picks; another choice reads `dashboard_picks`.
 */
const props = defineProps<{ view: MostPickedView }>()

const first = computed(() => props.view.picks.setCode ?? ALL_SETS)
const chosen = ref<string>(first.value)
// A new view (format or account switch, a sync) restarts from its own set.
watch(first, (value) => {
  chosen.value = value
})
const other = computed(() => chosen.value !== first.value)
const set = computed(() => (chosen.value === ALL_SETS ? null : chosen.value))
const { data } = useDashboardPicks(set, other)
const picks = computed(() => (other.value ? (data.value?.picks ?? []) : props.view.picks.picks))
</script>

<template>
  <HomeCard title="Most picked">
    <template #aside>
      <Select v-model="chosen">
        <SelectTrigger class="h-7 w-36 text-xs" aria-label="Set">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="choice in pickSetChoices(view.sets)" :key="choice.value" :value="choice.value">
            {{ choice.label }}
          </SelectItem>
        </SelectContent>
      </Select>
    </template>
    <p v-if="picks.length === 0" class="py-6 text-center text-sm text-muted-foreground">No picks for this set</p>
    <div v-else class="grid grid-cols-3 gap-2">
      <RouterLink
        v-for="pick in picks"
        :key="pick.catalogId"
        :to="{ path: '/cards', query: { search: limitedPickName(pick) } }"
        :title="limitedPickName(pick)"
        class="relative block"
      >
        <CardImage
          v-if="pick.image"
          :src="pick.image"
          :local="pick.localImage"
          :alt="limitedPickName(pick)"
          class="w-full rounded-md"
        />
        <span
          v-else
          class="flex aspect-[5/7] items-center justify-center rounded-md bg-muted p-1 text-center text-[10px] text-muted-foreground"
        >
          {{ limitedPickName(pick) }}
        </span>
        <span class="absolute right-1 bottom-1 rounded bg-black/70 px-1 text-[10px] font-semibold text-white tabular-nums">
          ×{{ pick.count }}
        </span>
      </RouterLink>
    </div>
  </HomeCard>
</template>
