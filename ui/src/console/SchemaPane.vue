<script setup lang="ts">
import { ref } from 'vue'
import type { ConsoleObject, SavedQuery } from '@/bindings'
import SchemaObject from './SchemaObject.vue'
import TuiPane from './TuiPane.vue'

// The Schema pane: the views, saved queries, examples and raw tables.
// Up and Down move between entries, Enter picks one (a view or table runs
// `SELECT * FROM` it; a saved query or example loads into the editor),
// Right and Left open and close a view's columns, Delete removes a saved
// query (after a prompt).
defineProps<{
  focused: boolean
  views: ConsoleObject[]
  rawTables: ConsoleObject[]
  saved: SavedQuery[]
  examples: { name: string; sql: string }[]
  error: string | null
}>()
const emit = defineEmits<{
  runObject: [name: string]
  loadSaved: [query: SavedQuery]
  deleteSaved: [name: string]
  loadExample: [example: { name: string; sql: string }]
}>()

const list = ref<HTMLElement | null>(null)
const open = ref<Set<string>>(new Set())

function toggle(name: string, to?: boolean) {
  const next = new Set(open.value)
  const opening = to ?? !next.has(name)
  if (opening) next.add(name)
  else next.delete(name)
  open.value = next
}

function entries(): HTMLElement[] {
  return Array.from(list.value?.querySelectorAll<HTMLElement>('[data-nav]') ?? [])
}

/** The entry focused last, to return to when Tab comes back to the pane. */
let lastEntry: HTMLElement | null = null

function onFocusin(event: FocusEvent) {
  const target = event.target as HTMLElement
  if (target.dataset && 'nav' in target.dataset) lastEntry = target
}

function focus() {
  const all = entries()
  const remembered = lastEntry && all.includes(lastEntry) ? lastEntry : null
  ;(remembered ?? all[0])?.focus()
}

function onKeydown(event: KeyboardEvent) {
  const target = event.target as HTMLElement
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    const all = entries()
    const at = all.indexOf(target)
    const next = all[event.key === 'ArrowDown' ? at + 1 : at - 1]
    if (next) next.focus()
    event.preventDefault()
    return
  }
  const object = target.dataset.object
  if (object && (event.key === 'ArrowRight' || event.key === 'ArrowLeft')) {
    toggle(object, event.key === 'ArrowRight')
    event.preventDefault()
    return
  }
  const savedName = target.dataset.saved
  if (savedName && (event.key === 'Delete' || event.key === 'Backspace')) {
    emit('deleteSaved', savedName)
    event.preventDefault()
  }
}

defineExpose({ focus })
</script>

<template>
  <TuiPane title="Schema" :focused="focused">
    <div ref="list" class="tui-schema" @keydown="onKeydown" @focusin="onFocusin">
      <p v-if="error" class="tui-error">{{ error }}</p>

      <h2 class="tui-heading">Views</h2>
      <SchemaObject
        v-for="object in views"
        :key="object.name"
        :object="object"
        :open="open.has(object.name)"
        @run="emit('runObject', object.name)"
        @toggle="toggle(object.name)"
      />

      <h2 class="tui-heading">Saved</h2>
      <p v-if="saved.length === 0" class="tui-entry-none">s saves the query</p>
      <div v-for="query in saved" :key="query.name" class="tui-entry-row">
        <button
          type="button"
          class="tui-entry"
          data-nav
          :data-saved="query.name"
          :title="query.sql"
          @click="emit('loadSaved', query)"
        >
          {{ query.name }}
        </button>
        <button
          type="button"
          class="tui-toggle tui-delete"
          tabindex="-1"
          :aria-label="`Delete ${query.name}`"
          @click="emit('deleteSaved', query.name)"
        >
          x
        </button>
      </div>

      <h2 class="tui-heading">Examples</h2>
      <button
        v-for="example in examples"
        :key="example.name"
        type="button"
        class="tui-entry"
        data-nav
        :title="example.sql"
        @click="emit('loadExample', example)"
      >
        {{ example.name }}
      </button>

      <h2 class="tui-heading">Raw (may change)</h2>
      <SchemaObject
        v-for="object in rawTables"
        :key="object.name"
        :object="object"
        :open="open.has(object.name)"
        @run="emit('runObject', object.name)"
        @toggle="toggle(object.name)"
      />
    </div>
  </TuiPane>
</template>
