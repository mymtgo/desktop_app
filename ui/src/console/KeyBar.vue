<script setup lang="ts">
import { ref, watch } from 'vue'
import type { ExportFormat } from '@/bindings'
import type { ConsoleAction } from './keys'
import type { Prompt, Status } from './useConsole'

// The bottom line: the key hints (each also a button), or the open prompt
// (a name to save under, CSV or JSON, a delete or a replace to confirm),
// and a short status note on the right.
const props = defineProps<{ prompt: Prompt | null; status: Status | null; busy: 'run' | 'export' | null }>()
const emit = defineEmits<{
  action: [action: ConsoleAction]
  save: [name: string]
  exportAs: [format: ExportFormat]
  confirmDelete: [name: string]
  confirmReplace: [name: string]
  closePrompt: []
}>()

const HINTS: { keys: string; label: string; action: ConsoleAction }[] = [
  { keys: 'ctrl+enter', label: 'run', action: 'run' },
  { keys: 'esc', label: 'cancel', action: 'cancel' },
  { keys: 'tab', label: 'pane', action: 'nextPane' },
  { keys: 's', label: 'save', action: 'save' },
  { keys: 'e', label: 'export', action: 'export' },
  { keys: 'c', label: 'chart', action: 'chart' },
  { keys: 'o', label: 'folder', action: 'folder' },
]

const nameField = ref<HTMLInputElement | null>(null)
/** The open export, delete or replace prompt: focused, so its letters answer it rather than type into the editor. */
const choice = ref<HTMLElement | null>(null)
const name = ref('')

watch(
  () => props.prompt,
  (prompt) => {
    if (prompt?.kind === 'save') name.value = prompt.name
  },
  { immediate: true },
)

watch(nameField, (field) => field?.focus())
watch(choice, (box) => box?.focus())

function onNameKeydown(event: KeyboardEvent) {
  if (event.key !== 'Enter' || event.ctrlKey || event.metaKey) return
  event.preventDefault()
  emit('save', name.value)
}
</script>

<template>
  <footer class="tui-keybar">
    <div v-if="prompt?.kind === 'save'" class="tui-prompt">
      <span class="tui-key">save as</span>
      <input
        ref="nameField"
        class="tui-field tui-prompt-field"
        :value="name"
        aria-label="Saved query name"
        spellcheck="false"
        @input="name = ($event.target as HTMLInputElement).value"
        @keydown="onNameKeydown"
      />
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('save', name)">
        <span class="tui-key">enter</span> save
      </button>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('closePrompt')">
        <span class="tui-key">esc</span> cancel
      </button>
    </div>
    <div v-else-if="prompt?.kind === 'export'" ref="choice" class="tui-prompt" tabindex="-1" data-prompt="export">
      <span>export as</span>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('exportAs', 'csv')">
        <span class="tui-key">c</span> csv
      </button>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('exportAs', 'json')">
        <span class="tui-key">j</span> json
      </button>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('closePrompt')">
        <span class="tui-key">esc</span> cancel
      </button>
    </div>
    <div v-else-if="prompt?.kind === 'delete'" ref="choice" class="tui-prompt" tabindex="-1" data-prompt="delete">
      <span>delete "{{ prompt.name }}"?</span>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('confirmDelete', prompt.name)">
        <span class="tui-key">y</span> yes
      </button>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('closePrompt')">
        <span class="tui-key">n</span> no
      </button>
    </div>
    <div v-else-if="prompt?.kind === 'replace'" ref="choice" class="tui-prompt" tabindex="-1" data-prompt="replace">
      <span>replace "{{ prompt.name }}"?</span>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('confirmReplace', prompt.name)">
        <span class="tui-key">y</span> yes
      </button>
      <button type="button" class="tui-hint" tabindex="-1" @click="emit('closePrompt')">
        <span class="tui-key">n</span> no
      </button>
    </div>
    <div v-else class="tui-prompt">
      <button
        v-for="hint in HINTS"
        :key="hint.action"
        type="button"
        class="tui-hint"
        tabindex="-1"
        :data-action="hint.action"
        @click="emit('action', hint.action)"
      >
        <span class="tui-key">{{ hint.keys }}</span> {{ hint.label }}
      </button>
    </div>
    <span v-if="busy === 'export'" class="tui-status tui-warn">exporting...  esc cancel</span>
    <span v-else-if="status" class="tui-status" :class="{ 'tui-error-text': status.error }">{{ status.text }}</span>
  </footer>
</template>
