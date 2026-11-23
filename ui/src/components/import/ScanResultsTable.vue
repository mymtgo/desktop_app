<script setup lang="ts">
import { Eye } from 'lucide-vue-next'
import { computed } from 'vue'
import type { HistoryScanMatch } from '@/bindings'
import { Badge } from '@/components/ui/badge'
import { Card, CardContent } from '@/components/ui/card'
import { confidenceColor, confidenceText, formatScanDate, outcomeVariant } from '@/lib/historyImport'

/** One page of scanned matches with selection (0.x results table). */
const props = defineProps<{
  rows: HistoryScanMatch[]
  selected: Set<number>
  emptyText: string
}>()

const emit = defineEmits<{
  toggle: [historyId: number]
  togglePage: []
  showCards: [match: HistoryScanMatch]
}>()

const allPageSelected = computed(() => props.rows.length > 0 && props.rows.every((m) => props.selected.has(m.historyId)))
</script>

<template>
  <Card class="overflow-hidden">
    <CardContent class="p-0">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b bg-muted/30 text-left text-xs font-medium text-muted-foreground">
            <th class="w-10 p-3">
              <input
                type="checkbox"
                :checked="allPageSelected"
                aria-label="Select this page"
                class="size-4 rounded border-input accent-primary"
                @change="emit('togglePage')"
              />
            </th>
            <th class="p-3">Date</th>
            <th class="p-3">Opponent</th>
            <th class="p-3">Format</th>
            <th class="p-3">Result</th>
            <th class="p-3 text-right">Confidence</th>
            <th class="w-10 p-3"></th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="match in rows"
            :key="match.historyId"
            class="border-b transition-colors last:border-b-0 hover:bg-muted/30"
            :class="{ 'bg-muted/10': selected.has(match.historyId) }"
          >
            <td class="w-10 p-3">
              <input
                type="checkbox"
                :checked="selected.has(match.historyId)"
                aria-label="Select match"
                class="size-4 rounded border-input accent-primary"
                @change="emit('toggle', match.historyId)"
              />
            </td>
            <td class="p-3 whitespace-nowrap text-muted-foreground">{{ formatScanDate(match.startedAtMs) }}</td>
            <td class="p-3">{{ match.opponent }}</td>
            <td class="p-3">
              <Badge variant="outline" class="text-xs">{{ match.formatDisplay }}</Badge>
            </td>
            <td class="p-3">
              <Badge :variant="outcomeVariant(match.outcome)" class="text-xs">
                {{ match.gamesWon }}-{{ match.gamesLost }}
              </Badge>
            </td>
            <td class="p-3 text-right">
              <span :class="confidenceColor(match.confidence)" class="text-xs font-medium">
                {{ confidenceText(match.confidence) }}
              </span>
            </td>
            <td class="w-10 p-3 text-center">
              <button
                v-if="match.gameLogToken"
                type="button"
                aria-label="Cards seen"
                class="inline-flex items-center rounded px-1.5 py-0.5 text-muted-foreground hover:text-foreground"
                @click="emit('showCards', match)"
              >
                <Eye class="size-3.5" />
              </button>
            </td>
          </tr>
          <tr v-if="rows.length === 0">
            <td colspan="7" class="p-8 text-center text-sm text-muted-foreground">{{ emptyText }}</td>
          </tr>
        </tbody>
      </table>
    </CardContent>
  </Card>
</template>
