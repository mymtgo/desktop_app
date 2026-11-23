<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from 'vue'
import { examples } from './examples'
import KeyBar from './KeyBar.vue'
import { actionFor, type ConsoleAction } from './keys'
import QueryPane from './QueryPane.vue'
import ResultsPane from './ResultsPane.vue'
import SchemaPane from './SchemaPane.vue'
import { useConsole } from './useConsole'
import './console.css'

// The data console window: three boxed panes (Schema, Query, Results) and
// the key bar, styled as a terminal app. Keys are read on the window.
const model = useConsole()
const {
  sql,
  paramNames,
  values,
  pickers,
  result,
  error,
  busy,
  views,
  rawTables,
  schemaError,
  saved,
  loadedName,
  prompt,
  status,
} = model

type Pane = 'schema' | 'query' | 'results'
const PANES: Pane[] = ['schema', 'query', 'results']
const focused = ref<Pane | null>(null)

const schemaPane = ref<InstanceType<typeof SchemaPane> | null>(null)
const queryPane = ref<InstanceType<typeof QueryPane> | null>(null)
const resultsPane = ref<InstanceType<typeof ResultsPane> | null>(null)

function focusPane(pane: Pane) {
  const target = { schema: schemaPane, query: queryPane, results: resultsPane }[pane]
  target.value?.focus()
}

function stepPane(step: 1 | -1) {
  const at = focused.value ? PANES.indexOf(focused.value) : -1
  const next = PANES[(at + step + PANES.length) % PANES.length]
  focusPane(next)
}

/** The element that had focus before a prompt opened, to return to when it closes. */
let beforePrompt: HTMLElement | null = null

function rememberFocus() {
  beforePrompt = (document.activeElement as HTMLElement | null) ?? null
}

function closePrompt() {
  prompt.value = null
  beforePrompt?.focus()
  beforePrompt = null
}

function perform(action: ConsoleAction) {
  switch (action) {
    case 'run':
      void model.run()
      break
    case 'cancel':
      if (prompt.value) closePrompt()
      else model.cancel()
      break
    case 'nextPane':
      stepPane(1)
      break
    case 'prevPane':
      stepPane(-1)
      break
    case 'save':
      rememberFocus()
      model.askSave()
      break
    case 'export':
      rememberFocus()
      model.askExport()
      break
    case 'chart':
      resultsPane.value?.toggleView()
      break
    case 'folder':
      void model.openFolder()
      break
  }
}

async function save(name: string) {
  await model.save(name)
  if (!prompt.value) closePrompt()
}

function exportAs(format: 'csv' | 'json') {
  closePrompt()
  void model.exportAs(format)
}

function confirmDelete(name: string) {
  closePrompt()
  void model.deleteSaved(name)
}

async function confirmReplace(name: string) {
  await model.save(name, true)
  if (!prompt.value) closePrompt()
}

function askDelete(name: string) {
  rememberFocus()
  model.askDelete(name)
}

function onKeydown(event: KeyboardEvent) {
  const command = event.ctrlKey || event.metaKey || event.altKey
  const tag = (event.target as HTMLElement | null)?.tagName
  const inEditor = tag === 'TEXTAREA' || tag === 'INPUT' || tag === 'SELECT'
  const open = prompt.value
  // A prompt's letters (c, j, y, n) are typed as usual in a text field; an
  // export, delete or replace prompt takes focus when it opens, so they
  // answer it.
  if (open && open.kind !== 'save' && !command && !inEditor) {
    const key = event.key.toLowerCase()
    if (open.kind === 'export' && (key === 'c' || key === 'j')) {
      event.preventDefault()
      exportAs(key === 'c' ? 'csv' : 'json')
      return
    }
    if (open.kind === 'delete' && (key === 'y' || key === 'n')) {
      event.preventDefault()
      if (key === 'y') confirmDelete(open.name)
      else closePrompt()
      return
    }
    if (open.kind === 'replace' && (key === 'y' || key === 'n')) {
      event.preventDefault()
      if (key === 'y') void confirmReplace(open.name)
      else closePrompt()
      return
    }
  }
  const action = actionFor(event, inEditor)
  if (!action) return
  event.preventDefault()
  perform(action)
}

onMounted(() => {
  window.addEventListener('keydown', onKeydown)
  void model.loadSchema()
  void model.loadSavedList()
  queryPane.value?.focus()
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
})
</script>

<template>
  <div class="tui-root">
    <div class="tui-area-schema" @focusin="focused = 'schema'">
      <SchemaPane
        ref="schemaPane"
        :focused="focused === 'schema'"
        :views="views"
        :raw-tables="rawTables"
        :saved="saved"
        :examples="examples"
        :error="schemaError"
        @run-object="model.runObject"
        @load-saved="model.loadSaved"
        @delete-saved="askDelete"
        @load-example="model.loadExample"
      />
    </div>
    <div class="tui-area-query" @focusin="focused = 'query'">
      <QueryPane
        ref="queryPane"
        :sql="sql"
        :focused="focused === 'query'"
        :loaded-name="loadedName"
        @update:sql="model.setSql"
      />
    </div>
    <div class="tui-area-results" @focusin="focused = 'results'">
      <ResultsPane
        ref="resultsPane"
        :focused="focused === 'results'"
        :result="result"
        :error="error"
        :busy="busy"
        :param-names="paramNames"
        :values="values"
        :pickers="pickers"
        @change="model.setValue"
      />
    </div>
    <KeyBar
      class="tui-area-keys"
      :prompt="prompt"
      :status="status"
      :busy="busy"
      @action="perform"
      @save="save"
      @export-as="exportAs"
      @confirm-delete="confirmDelete"
      @confirm-replace="confirmReplace"
      @close-prompt="closePrompt"
    />
  </div>
</template>
