<script setup lang="ts">
import { ImageOff } from 'lucide-vue-next'
import { computed } from 'vue'
import type { CardStatView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import WinPct from '@/components/cards/WinPct.vue'
import { Card } from '@/components/ui/card'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { CARD_STATS_COLUMNS, CASTING_METHOD_COLUMNS, type CardStatsColumnKey, type Visibility } from '@/lib/cardStats/columns'
import { estimatedKind, estimatedTooltip, inferredTooltip, pct, rawWinPctLabel, shrunkWinPct, winRateClass } from '@/lib/cardStats/format'
import type { Perspective, ShrinkKey } from '@/lib/cardStats/shrinkage'

const props = defineProps<{
  stat: CardStatView
  shrunk: Readonly<Record<ShrinkKey, number>>
  rawRates: Readonly<Record<ShrinkKey, number | null>>
  samples: Readonly<Record<ShrinkKey, number>>
  prior: number
  trust: number
  visibleColumns: Visibility
  /** Games from MTGO's game log alone: Seen and the sideboard numbers are estimates. */
  logOnlyGames?: number
  perspective: Perspective
}>()

/** At most this many visible stat columns, in column order, fill the tile beside the image. */
const MAX_FIELDS = 8

type Render =
  | { kind: 'winpct'; shrinkKey: ShrinkKey }
  | { kind: 'pct'; value: number | null; count: number; inferred?: number }
  | { kind: 'count'; value: number; zeroDash: boolean }

type Field = { key: CardStatsColumnKey; label: string; render: Render }

function build(key: CardStatsColumnKey): Omit<Field, 'key'> | null {
  const s = props.stat
  switch (key) {
    case 'keptPct':
      return { label: 'Kept', render: { kind: 'pct', value: pct(s.keptGames, s.totalGames), count: s.keptGames } }
    case 'keptWinPct':
      return { label: 'Kept win', render: { kind: 'winpct', shrinkKey: 'kept' } }
    case 'castPct':
      return { label: 'Cast', render: { kind: 'pct', value: pct(s.castGames, s.totalGames), count: s.castGames } }
    case 'castWinPct':
      return { label: 'Cast win', render: { kind: 'winpct', shrinkKey: 'cast' } }
    case 'playedPct':
      return { label: 'Played', render: { kind: 'pct', value: pct(s.playedGames, s.totalGames), count: s.playedGames } }
    case 'kicked':
      return { label: 'Kicked', render: { kind: 'count', value: s.totalKicked, zeroDash: true } }
    case 'activated':
      return { label: 'Activated', render: { kind: 'count', value: s.totalActivated, zeroDash: true } }
    case 'pregamePct':
      return { label: 'Pregame', render: { kind: 'pct', value: pct(s.pregameGames, s.totalGames), count: s.pregameGames } }
    case 'pregameWinPct':
      return { label: 'Pregame win', render: { kind: 'winpct', shrinkKey: 'pregame' } }
    case 'seenPct':
      return { label: 'Seen', render: { kind: 'pct', value: pct(s.seenGames, s.totalGames), count: s.seenGames } }
    case 'seenWinPct':
      return { label: 'Seen win', render: { kind: 'winpct', shrinkKey: 'seen' } }
    case 'sbOutPct':
      return {
        label: 'SB out',
        render: { kind: 'pct', value: pct(s.sidedOutGames, s.postboardGames), count: s.sidedOutGames },
      }
    case 'sbInPct':
      return {
        label: 'SB in',
        render: {
          kind: 'pct',
          value: pct(s.sidedInGames, s.postboardGames),
          count: s.sidedInGames,
          inferred: s.inferredGames,
        },
      }
    case 'games':
      return { label: 'Games', render: { kind: 'count', value: s.totalGames, zeroDash: false } }
    default: {
      const casting = CASTING_METHOD_COLUMNS.find((col) => col.key === key)
      if (casting) return { label: casting.label, render: { kind: 'count', value: s[casting.statField], zeroDash: true } }
      // `type` and `sb` carry no value in a tile.
      return null
    }
  }
}

const fields = computed<Field[]>(() => {
  const out: Field[] = []
  for (const col of CARD_STATS_COLUMNS) {
    if (!props.visibleColumns[col.key]) continue
    const field = build(col.key)
    if (!field) continue
    out.push({ key: col.key, ...field })
    if (out.length >= MAX_FIELDS) break
  }
  return out
})

const winPct = (key: ShrinkKey) => shrunkWinPct(props.shrunk, key)
const winLabel = (key: ShrinkKey) =>
  rawWinPctLabel(props.rawRates, props.samples, key, props.trust, props.prior, props.perspective)
const winColour = (key: ShrinkKey) => winRateClass(winPct(key), props.perspective)
</script>

<template>
  <Card class="flex flex-row gap-3 p-3">
    <div class="relative aspect-[488/680] w-48 shrink-0 overflow-hidden rounded-md bg-muted">
      <CardImage
        v-if="stat.image || stat.localImage"
        :src="stat.image"
        :local="stat.localImage"
        :alt="stat.name"
        class="size-full object-cover"
      />
      <div v-else class="flex size-full items-center justify-center text-muted-foreground/40">
        <ImageOff class="size-5" />
      </div>
    </div>

    <div class="flex min-w-0 flex-1 flex-col gap-1.5">
      <div class="flex items-start gap-1.5">
        <span class="min-w-0 flex-1 truncate text-sm font-medium" :title="stat.name">{{ stat.name }}</span>
        <span
          v-if="stat.isSideboard"
          class="shrink-0 rounded border border-black/40 bg-muted px-1 py-0.5 text-[10px] leading-none text-muted-foreground"
        >
          SB
        </span>
      </div>

      <dl class="flex flex-col gap-y-1 text-xs tabular-nums">
        <div v-for="field in fields" :key="field.key" class="flex items-baseline justify-between gap-2">
          <dt class="truncate text-muted-foreground">
            {{ field.label }}
            <Tooltip v-if="logOnlyGames && estimatedKind(field.key)">
              <TooltipTrigger as-child>
                <sup class="cursor-default">est.</sup>
              </TooltipTrigger>
              <TooltipContent side="top" class="max-w-xs text-xs">
                {{ estimatedTooltip(estimatedKind(field.key) ?? 'seen', logOnlyGames) }}
              </TooltipContent>
            </Tooltip>
          </dt>
          <dd class="shrink-0 text-right">
            <WinPct
              v-if="field.render.kind === 'winpct'"
              :samples="samples[field.render.shrinkKey]"
              :value="winPct(field.render.shrinkKey)"
              :colour="winColour(field.render.shrinkKey)"
              :tooltip="winLabel(field.render.shrinkKey)"
            />
            <template v-else-if="field.render.kind === 'pct'">
              <template v-if="field.render.value !== null">
                {{ field.render.value }}% <span class="text-[10px] text-muted-foreground">({{ field.render.count }})</span>
                <Tooltip v-if="field.render.inferred">
                  <TooltipTrigger as-child>
                    <sup class="ml-0.5 cursor-default text-muted-foreground" aria-label="Includes imported games">*</sup>
                  </TooltipTrigger>
                  <TooltipContent side="top" class="max-w-xs text-xs">
                    {{ inferredTooltip(field.render.inferred) }}
                  </TooltipContent>
                </Tooltip>
              </template>
              <span v-else class="text-muted-foreground">-</span>
            </template>
            <template v-else>
              <template v-if="!field.render.zeroDash || field.render.value > 0">{{ field.render.value }}</template>
              <span v-else class="text-muted-foreground">-</span>
            </template>
          </dd>
        </div>
      </dl>
    </div>
  </Card>
</template>
