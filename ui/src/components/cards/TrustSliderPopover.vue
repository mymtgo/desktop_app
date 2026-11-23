<script setup lang="ts">
import { Sliders } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'

defineProps<{ modelValue: number; max: number }>()

const emit = defineEmits<{
  'update:modelValue': [value: number]
  reset: []
}>()
</script>

<template>
  <Popover>
    <PopoverTrigger as-child>
      <Button
        variant="ghost"
        size="sm"
        class="bevel gap-1.5 border border-black/60 px-2.5 py-4 text-xs text-muted-foreground"
      >
        <Sliders class="size-3.5" />
        <span class="hidden lg:inline">Trust</span>
        <span class="hidden text-foreground/80 tabular-nums lg:inline">{{ modelValue }}</span>
      </Button>
    </PopoverTrigger>
    <PopoverContent align="end" class="w-72">
      <div class="flex flex-col gap-3">
        <div class="flex items-baseline justify-between">
          <span class="text-sm font-semibold">Games to trust</span>
          <span class="text-xs text-muted-foreground tabular-nums">{{ modelValue }}</span>
        </div>
        <p class="text-xs text-muted-foreground">
          How many games a card needs before its win rate stops getting pulled toward the deck baseline. Lower values
          trust small samples more; higher values keep flaky cards close to baseline.
        </p>
        <input
          type="range"
          min="0"
          :max="max"
          step="1"
          :value="modelValue"
          aria-label="Games to trust"
          class="w-full accent-primary"
          @input="emit('update:modelValue', Number(($event.target as HTMLInputElement).value))"
        />
        <div class="flex items-center justify-between text-[10px] text-muted-foreground">
          <span>Trust raw data</span>
          <span>Be skeptical</span>
        </div>
        <div class="flex justify-end">
          <Button variant="ghost" size="sm" class="h-7 px-2 text-xs" @click="emit('reset')">Reset to 50</Button>
        </div>
      </div>
    </PopoverContent>
  </Popover>
</template>
