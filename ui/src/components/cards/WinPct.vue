<script setup lang="ts">
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'

// A shrunk win rate with its sample size and the raw numbers behind it
// (0.x showed `-` without samples). Colours come from `winRateClass`.
defineProps<{ samples: number; value: number; colour: string; tooltip: string }>()
</script>

<template>
  <Tooltip v-if="samples > 0">
    <TooltipTrigger as-child>
      <span class="cursor-default border-b border-dotted border-muted-foreground/40">
        <span class="font-medium" :class="colour">{{ value }}%</span>
        <span class="text-[10px] text-muted-foreground">({{ samples }})</span>
      </span>
    </TooltipTrigger>
    <TooltipContent side="top" class="text-xs">{{ tooltip }}</TooltipContent>
  </Tooltip>
  <span v-else class="text-muted-foreground">-</span>
</template>
