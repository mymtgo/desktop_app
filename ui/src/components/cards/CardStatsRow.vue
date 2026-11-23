<script setup lang="ts">
import { Check, Image } from 'lucide-vue-next'
import { computed } from 'vue'
import type { CardStatView } from '@/bindings'
import WinPct from '@/components/cards/WinPct.vue'
import { TableCell, TableRow } from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { CASTING_METHOD_COLUMNS, type Visibility } from '@/lib/cardStats/columns'
import { inferredTooltip, pct, rawWinPctLabel, shrunkWinPct, winRateClass } from '@/lib/cardStats/format'
import type { Perspective, ShrinkKey } from '@/lib/cardStats/shrinkage'

const props = defineProps<{
  stat: CardStatView
  shrunk: Readonly<Record<ShrinkKey, number>>
  rawRates: Readonly<Record<ShrinkKey, number | null>>
  samples: Readonly<Record<ShrinkKey, number>>
  prior: number
  trust: number
  visibleColumns: Visibility
  perspective: Perspective
}>()

const emit = defineEmits<{
  imageEnter: [stat: CardStatView]
  imageMove: [event: MouseEvent]
  imageLeave: []
}>()

/** The alternative casting methods, shown under Cast % (flashback, madness and evoke have no column of their own). */
const ALT_COST_LABELS: [keyof CardStatView, string][] = [
  ['totalFlashback', 'flashback'],
  ['totalMadness', 'madness'],
  ['totalEvoked', 'evoke'],
  ['totalWarp', 'warp'],
  ['totalFreeCast', 'free'],
  ['totalBargained', 'bargain'],
  ['totalDashed', 'dash'],
  ['totalBestowed', 'bestow'],
  ['totalReplicated', 'replicate'],
  ['totalSpectacle', 'spectacle'],
  ['totalRebound', 'rebound'],
  ['totalEscaped', 'escape'],
  ['totalNinjutsu', 'ninjutsu'],
  ['totalSuspended', 'suspend'],
  ['totalBuyback', 'buyback'],
  ['totalDisturb', 'disturb'],
  ['totalForetold', 'foretell'],
  ['totalRetraced', 'retrace'],
  ['totalMayhem', 'mayhem'],
  ['totalMiracle', 'miracle'],
  ['totalGifted', 'gift'],
  ['totalCasualty', 'casualty'],
]

const altCostBreakdown = computed(() =>
  ALT_COST_LABELS.filter(([key]) => (props.stat[key] as number) > 0)
    .map(([key, label]) => `${props.stat[key]} ${label}`)
    .join(', '),
)

const winPct = (key: ShrinkKey) => shrunkWinPct(props.shrunk, key)
const winLabel = (key: ShrinkKey) =>
  rawWinPctLabel(props.rawRates, props.samples, key, props.trust, props.prior, props.perspective)
const winColour = (key: ShrinkKey) => winRateClass(winPct(key), props.perspective)

const kept = computed(() => pct(props.stat.keptGames, props.stat.totalGames))
const cast = computed(() => pct(props.stat.castGames, props.stat.totalGames))
const played = computed(() => pct(props.stat.playedGames, props.stat.totalGames))
const pregame = computed(() => (props.stat.pregameGames > 0 ? pct(props.stat.pregameGames, props.stat.totalGames) : null))
const seen = computed(() => pct(props.stat.seenGames, props.stat.totalGames))
const sbOut = computed(() => pct(props.stat.sidedOutGames, props.stat.postboardGames))
const sbIn = computed(() => pct(props.stat.sidedInGames, props.stat.postboardGames))
</script>

<template>
  <TableRow>
    <TableCell class="font-medium">
      <span class="flex items-center gap-1.5">
        <Image
          v-if="stat.image || stat.localImage"
          class="size-3.5 shrink-0 cursor-pointer text-zinc-600 hover:text-zinc-400"
          @mouseenter="emit('imageEnter', stat)"
          @mousemove="(e: MouseEvent) => emit('imageMove', e)"
          @mouseleave="emit('imageLeave')"
        />
        {{ stat.name }}
      </span>
    </TableCell>
    <TableCell v-if="visibleColumns.type" class="text-muted-foreground">{{ stat.type ?? '-' }}</TableCell>
    <TableCell v-if="visibleColumns.sb" class="text-center">
      <Check v-if="stat.isSideboard" class="mx-auto size-3.5 text-muted-foreground" />
    </TableCell>
    <TableCell v-if="visibleColumns.keptPct" class="text-right tabular-nums">
      <template v-if="kept !== null">
        {{ kept }}% <span class="text-[10px] text-muted-foreground">({{ stat.keptGames }})</span>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.keptWinPct" class="text-right tabular-nums">
      <WinPct :samples="samples.kept" :value="winPct('kept')" :colour="winColour('kept')" :tooltip="winLabel('kept')" />
    </TableCell>
    <TableCell v-if="visibleColumns.castPct" class="text-right tabular-nums">
      <template v-if="cast !== null">
        <Tooltip v-if="altCostBreakdown">
          <TooltipTrigger as-child>
            <span class="cursor-default border-b border-dotted border-muted-foreground">
              {{ cast }}% <span class="text-[10px] text-muted-foreground">({{ stat.castGames }})</span>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top" class="text-xs">{{ altCostBreakdown }}</TooltipContent>
        </Tooltip>
        <template v-else>
          {{ cast }}% <span class="text-[10px] text-muted-foreground">({{ stat.castGames }})</span>
        </template>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.castWinPct" class="text-right tabular-nums">
      <WinPct :samples="samples.cast" :value="winPct('cast')" :colour="winColour('cast')" :tooltip="winLabel('cast')" />
    </TableCell>
    <TableCell v-if="visibleColumns.playedPct" class="text-right tabular-nums">
      <template v-if="played !== null">
        {{ played }}% <span class="text-[10px] text-muted-foreground">({{ stat.playedGames }})</span>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.kicked" class="text-right tabular-nums">
      <template v-if="stat.totalKicked > 0">{{ stat.totalKicked }}</template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.activated" class="text-right tabular-nums">
      <template v-if="stat.totalActivated > 0">{{ stat.totalActivated }}</template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <template v-for="column in CASTING_METHOD_COLUMNS" :key="column.key">
      <TableCell v-if="visibleColumns[column.key]" class="text-right tabular-nums">
        <template v-if="stat[column.statField] > 0">{{ stat[column.statField] }}</template>
        <span v-else class="text-muted-foreground">-</span>
      </TableCell>
    </template>
    <TableCell v-if="visibleColumns.pregamePct" class="text-right tabular-nums">
      <template v-if="pregame !== null">
        <Tooltip v-if="stat.pregameRevealedGames > 0 && stat.pregamePlayedGames > 0">
          <TooltipTrigger as-child>
            <span class="cursor-default border-b border-dotted border-muted-foreground">
              {{ pregame }}% <span class="text-[10px] text-muted-foreground">({{ stat.pregameGames }})</span>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top" class="text-xs">
            {{ stat.pregameRevealedGames }} revealed, {{ stat.pregamePlayedGames }} played
          </TooltipContent>
        </Tooltip>
        <template v-else>
          {{ pregame }}% <span class="text-[10px] text-muted-foreground">({{ stat.pregameGames }})</span>
        </template>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.pregameWinPct" class="text-right tabular-nums">
      <WinPct
        :samples="samples.pregame"
        :value="winPct('pregame')"
        :colour="winColour('pregame')"
        :tooltip="winLabel('pregame')"
      />
    </TableCell>
    <TableCell v-if="visibleColumns.seenPct" class="text-right tabular-nums">
      <template v-if="seen !== null">
        {{ seen }}% <span class="text-[10px] text-muted-foreground">({{ stat.seenGames }})</span>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.seenWinPct" class="text-right tabular-nums">
      <WinPct :samples="samples.seen" :value="winPct('seen')" :colour="winColour('seen')" :tooltip="winLabel('seen')" />
    </TableCell>
    <TableCell v-if="visibleColumns.sbOutPct" class="text-right tabular-nums">
      <template v-if="sbOut !== null">
        {{ sbOut }}% <span class="text-[10px] text-muted-foreground">({{ stat.sidedOutGames }})</span>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.sbInPct" class="text-right tabular-nums">
      <template v-if="sbIn !== null">
        {{ sbIn }}% <span class="text-[10px] text-muted-foreground">({{ stat.sidedInGames }})</span>
        <Tooltip v-if="stat.inferredGames > 0">
          <TooltipTrigger as-child>
            <sup class="ml-0.5 cursor-default text-muted-foreground" aria-label="Includes imported games">*</sup>
          </TooltipTrigger>
          <TooltipContent side="top" class="max-w-xs text-xs">{{ inferredTooltip(stat.inferredGames) }}</TooltipContent>
        </Tooltip>
      </template>
      <span v-else class="text-muted-foreground">-</span>
    </TableCell>
    <TableCell v-if="visibleColumns.games" class="text-right text-muted-foreground tabular-nums">
      {{ stat.totalGames }}
    </TableCell>
  </TableRow>
</template>
