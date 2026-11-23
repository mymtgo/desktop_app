<script setup lang="ts">
import { ref } from 'vue'
import TuiPane from './TuiPane.vue'

// The Query pane: a plain monospace editor. `:value` and `@input` rather
// than `v-model`, so the text stays in the console's one copy.
defineProps<{ sql: string; focused: boolean; loadedName: string | null }>()
const emit = defineEmits<{ 'update:sql': [value: string] }>()

const editor = ref<HTMLTextAreaElement | null>(null)

function onInput(event: Event) {
  emit('update:sql', (event.target as HTMLTextAreaElement).value)
}

defineExpose({ focus: () => editor.value?.focus() })
</script>

<template>
  <TuiPane title="Query" :focused="focused">
    <template v-if="loadedName" #title>
      <span class="tui-dim">{{ loadedName }}</span>
    </template>
    <textarea
      ref="editor"
      class="tui-editor"
      :value="sql"
      spellcheck="false"
      autocapitalize="off"
      autocomplete="off"
      aria-label="SQL query"
      placeholder="SELECT * FROM v_matches"
      @input="onInput"
    />
  </TuiPane>
</template>
