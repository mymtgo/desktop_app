<script setup lang="ts">
import type { ConsoleObject } from '@/bindings'

// One view or table in the Schema pane: its name (Enter runs it) and,
// when open, its columns with their types and descriptions.
defineProps<{ object: ConsoleObject; open: boolean }>()
const emit = defineEmits<{ run: []; toggle: [] }>()
</script>

<template>
  <div>
    <div class="tui-entry-row">
      <button
        type="button"
        class="tui-toggle"
        tabindex="-1"
        :aria-label="open ? `Hide ${object.name} columns` : `Show ${object.name} columns`"
        @click="emit('toggle')"
      >
        {{ open ? '▾' : '▸' }}
      </button>
      <button
        type="button"
        class="tui-entry"
        data-nav
        :data-object="object.name"
        :aria-expanded="open"
        @click="emit('run')"
      >
        {{ object.name }}
      </button>
    </div>
    <ul v-if="open" class="tui-columns">
      <li v-for="column in object.columns" :key="column.name">
        <span>{{ column.name }}</span>
        <span v-if="column.declType" class="tui-dim"> {{ column.declType }}</span>
        <span v-if="column.description" class="tui-column-help">{{ column.description }}</span>
      </li>
    </ul>
  </div>
</template>
