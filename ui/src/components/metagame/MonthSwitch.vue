<script setup lang="ts">
// The Arena metagame's months (seasons are calendar months), newest first.
import type { MetagameMonthView } from '@/bindings'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'

defineProps<{ months: MetagameMonthView[] }>()
const model = defineModel<string>({ required: true })
</script>

<template>
  <Select v-model="model">
    <SelectTrigger class="w-44" aria-label="Month">
      <SelectValue />
    </SelectTrigger>
    <SelectContent>
      <SelectItem v-for="month in months" :key="month.key" :value="month.key">{{ month.label }}</SelectItem>
    </SelectContent>
  </Select>
</template>
