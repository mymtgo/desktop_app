<script setup lang="ts">
import { Castle, Check, Filter, Flame, Gem, HandFist, MountainSnow, Origami, PanelRightOpen, ScrollText, Zap } from 'lucide-vue-next'
import { computed, type Component } from 'vue'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { FilterKey, TypeFilters } from '@/lib/cardStats/typeFilter'

const props = defineProps<{
  modelValue: TypeFilters
  /** The toggles to show, in order. */
  keys: FilterKey[]
}>()

const emit = defineEmits<{ 'update:modelValue': [value: TypeFilters] }>()

const OPTIONS: Record<FilterKey, { label: string; icon: Component }> = {
  Creature: { label: 'Creatures', icon: Origami },
  Instant: { label: 'Instants', icon: Zap },
  Sorcery: { label: 'Sorceries', icon: Flame },
  Enchantment: { label: 'Enchantments', icon: ScrollText },
  Artifact: { label: 'Artifacts', icon: Gem },
  Land: { label: 'Lands', icon: MountainSnow },
  Planeswalker: { label: 'Planeswalkers', icon: HandFist },
  Battle: { label: 'Battles', icon: Castle },
  Sideboard: { label: 'Sideboard', icon: PanelRightOpen },
}

const hiddenCount = computed(() => props.keys.filter((key) => !props.modelValue[key]).length)
const allVisible = computed(() => props.keys.every((key) => props.modelValue[key]))

function setFilter(key: FilterKey, value: boolean) {
  emit('update:modelValue', { ...props.modelValue, [key]: value })
}

function toggleAll() {
  const next = !allVisible.value
  const updated = { ...props.modelValue }
  for (const key of props.keys) updated[key] = next
  emit('update:modelValue', updated)
}
</script>

<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <Button variant="outline" size="sm" class="bevel h-8 gap-1.5 border-black/60 text-xs">
        <Filter class="size-3.5" />
        <span v-if="hiddenCount > 0">{{ hiddenCount }} hidden</span>
        <span v-else>Card types</span>
      </Button>
    </DropdownMenuTrigger>
    <DropdownMenuContent align="end" class="w-48">
      <div class="flex items-center justify-between px-2 py-1.5">
        <span class="text-xs font-semibold">Filter by type</span>
        <button type="button" class="text-xs text-muted-foreground hover:text-foreground" @click="toggleAll">
          {{ allVisible ? 'Hide all' : 'Show all' }}
        </button>
      </div>
      <DropdownMenuSeparator />
      <template v-for="key in keys" :key="key">
        <DropdownMenuSeparator v-if="key === 'Sideboard'" />
        <DropdownMenuCheckboxItem
          :model-value="!!modelValue[key]"
          @update:model-value="(value: boolean) => setFilter(key, value)"
          @select.prevent
        >
          <template #indicator-icon><Check class="size-4 text-success" /></template>
          <component :is="OPTIONS[key].icon" class="mr-2 size-3.5 text-muted-foreground" />
          {{ OPTIONS[key].label }}
        </DropdownMenuCheckboxItem>
      </template>
    </DropdownMenuContent>
  </DropdownMenu>
</template>
