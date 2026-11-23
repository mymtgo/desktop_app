<script setup lang="ts">
import { pickerKind } from './params'
import type { PickerOptions } from './useConsole'

// The query's named parameters, one field each: a picker of archetype or
// deck names, or a text field (also when a picker's names could not load).
// An empty field is sent as NULL. A value the picker does not list (a saved
// query's deck since renamed) stays as an extra choice. A picker cut at the
// run cap says it shows the first 1,000 names.
const props = defineProps<{
  names: string[]
  values: Record<string, string>
  pickers: { archetype: PickerOptions; deck: PickerOptions }
}>()
const emit = defineEmits<{ change: [name: string, value: string] }>()

function loaded(name: string) {
  const kind = pickerKind(name)
  if (kind === 'text') return null
  const found = props.pickers[kind]
  return found && found !== 'failed' ? found : null
}

function options(name: string): string[] | null {
  const found = loaded(name)
  if (!found) return null
  const current = props.values[name] ?? ''
  return current === '' || found.names.includes(current) ? found.names : [current, ...found.names]
}

/** Whether the picker shows only the first names (the run cap cut the list). */
function truncated(name: string): boolean {
  return loaded(name)?.truncated ?? false
}

function onChange(name: string, event: Event) {
  emit('change', name, (event.target as HTMLInputElement | HTMLSelectElement).value)
}
</script>

<template>
  <div class="tui-params">
    <label v-for="name in names" :key="name" class="tui-param">
      <span class="tui-param-name">:{{ name }}</span>
      <template v-if="options(name)">
        <select class="tui-field" :data-param="name" :value="values[name] ?? ''" @change="onChange(name, $event)">
          <option value="">NULL</option>
          <option v-for="option in options(name)" :key="option" :value="option">{{ option }}</option>
        </select>
        <span v-if="truncated(name)" class="tui-dim" data-note="truncated">first 1,000 names</span>
      </template>
      <input
        v-else
        class="tui-field"
        :data-param="name"
        :value="values[name] ?? ''"
        placeholder="NULL"
        spellcheck="false"
        @input="onChange(name, $event)"
      />
    </label>
  </div>
</template>
