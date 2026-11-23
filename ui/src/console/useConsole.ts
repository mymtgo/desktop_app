import { computed, onBeforeUnmount, reactive, ref, shallowRef, watch } from 'vue'
import { commands, type ConsoleObject, type ConsoleResult, type ExportFormat, type SavedQuery } from '@/bindings'
import { paramValues, pickerKind, pickerSql, type PickerKind } from './params'

/**
 * A picker's choices: loading (`null`), the names (`truncated` when the run
 * cap cut the list), or `failed` (shown as a text field).
 */
export type PickerOptions = { names: string[]; truncated: boolean } | null | 'failed'

/**
 * The one-line prompt in the key bar: saving under a name, picking an
 * export format, confirming a delete, or confirming a save over another
 * saved query.
 */
export type Prompt =
  | { kind: 'save'; name: string }
  | { kind: 'export' }
  | { kind: 'delete'; name: string }
  | { kind: 'replace'; name: string }

/** A short note in the key bar (export done, saved, or an error). */
export type Status = { text: string; error: boolean }

/** How long the editor waits after typing before it lists the query's parameters again. */
const PARAMS_DELAY_MS = 400

/**
 * The data console's state and its commands: the query text, its named
 * parameters and their values, the last result or error, the schema and
 * the saved queries. Components read it and call its functions.
 */
export function useConsole() {
  const sql = ref('')
  const paramNames = ref<string[]>([])
  const values = reactive<Record<string, string>>({})
  const pickers = reactive<Record<Exclude<PickerKind, 'text'>, PickerOptions>>({ archetype: null, deck: null })
  const result = shallowRef<ConsoleResult | null>(null)
  const error = ref<string | null>(null)
  const busy = ref<'run' | 'export' | null>(null)
  const schema = shallowRef<ConsoleObject[]>([])
  const schemaError = ref<string | null>(null)
  const saved = shallowRef<SavedQuery[]>([])
  const loadedName = ref<string | null>(null)
  const prompt = ref<Prompt | null>(null)
  const status = ref<Status | null>(null)

  let latestRun = 0
  /** The picker loads in flight, so overlapping refreshes share one fetch. */
  const pickerLoads: Partial<Record<Exclude<PickerKind, 'text'>, Promise<void>>> = {}
  let paramsTimer: ReturnType<typeof setTimeout> | null = null
  let paramsAsked = 0

  async function loadSchema() {
    const answer = await commands.consoleSchema()
    if (answer.status === 'ok') schema.value = answer.data
    else schemaError.value = answer.error
  }

  async function loadSavedList() {
    const answer = await commands.consoleSavedList()
    if (answer.status === 'ok') saved.value = answer.data
    else status.value = { text: answer.error, error: true }
  }

  /**
   * Loads a picker's names, once unless `reload` (each run reloads them, so
   * a new deck or archetype shows up, and a failed load is tried again).
   * The old names stay shown while the new ones load.
   */
  function loadPicker(kind: Exclude<PickerKind, 'text'>, reload = false): Promise<void> {
    const inFlight = pickerLoads[kind]
    if (inFlight) return inFlight
    if (pickers[kind] !== null && !reload) return Promise.resolve()
    const query = pickerSql(kind)
    if (!query) return Promise.resolve()
    const load = (async () => {
      try {
        const answer = await commands.consoleRun(query, [])
        if (answer.status !== 'ok') {
          if (pickers[kind] === null) pickers[kind] = 'failed'
          return
        }
        const names: string[] = []
        for (const row of answer.data.rows) {
          const cell = row[0]
          if (cell && cell.kind === 'text' && cell.value !== '') names.push(cell.value)
        }
        pickers[kind] = { names, truncated: answer.data.truncated }
      } catch {
        if (pickers[kind] === null) pickers[kind] = 'failed'
      } finally {
        delete pickerLoads[kind]
      }
    })()
    pickerLoads[kind] = load
    return load
  }

  /**
   * Lists the parameters of `text` (SQLite reports them), shows their fields
   * unless a later refresh already answered, and loads any pickers they need
   * (`reloadPickers` fetches their names again). Returns the names for
   * `text`: what a run, save or export of that text sends. A query that does
   * not parse yet keeps the fields it had and returns them, or no names when
   * `forSave` (a save must not store another query's names; a run or export
   * shows the parse error instead).
   */
  async function refreshParams(text: string, reloadPickers = false, forSave = false): Promise<string[]> {
    const asked = ++paramsAsked
    let names: string[]
    if (!/[:@$?]/.test(text)) {
      names = []
    } else {
      const answer = await commands.consoleParameters(text)
      if (answer.status !== 'ok') return forSave ? [] : paramNames.value
      names = answer.data
    }
    if (asked === paramsAsked) paramNames.value = names
    for (const name of names) {
      const kind = pickerKind(name)
      if (kind !== 'text') void loadPicker(kind, reloadPickers)
    }
    return names
  }

  watch(sql, () => {
    if (paramsTimer) clearTimeout(paramsTimer)
    paramsTimer = setTimeout(() => void refreshParams(sql.value), PARAMS_DELAY_MS)
  })

  onBeforeUnmount(() => {
    if (paramsTimer) clearTimeout(paramsTimer)
  })

  function setSql(text: string) {
    sql.value = text
  }

  function setValue(name: string, value: string) {
    values[name] = value
  }

  /** Runs the query as it is now: typing during the run changes nothing sent. */
  async function run() {
    const text = sql.value
    if (busy.value || text.trim() === '') return
    const id = ++latestRun
    busy.value = 'run'
    error.value = null
    try {
      if (paramsTimer) clearTimeout(paramsTimer)
      const names = await refreshParams(text, true)
      const answer = await commands.consoleRun(text, paramValues(names, values))
      if (id !== latestRun) return
      if (answer.status === 'ok') {
        result.value = answer.data
      } else {
        result.value = null
        error.value = answer.error
      }
    } finally {
      if (id === latestRun) busy.value = null
    }
  }

  /** Esc: stops the running query or export. */
  function cancel() {
    if (!busy.value) return
    commands.consoleCancel().catch((reason: unknown) => {
      status.value = { text: `Could not cancel: ${String(reason)}`, error: true }
    })
  }

  function runObject(name: string) {
    if (busy.value) {
      status.value = { text: 'Wait for the current query, or esc to cancel it.', error: false }
      return
    }
    sql.value = `SELECT * FROM ${name}`
    loadedName.value = null
    void run()
  }

  function loadExample(example: { name: string; sql: string }) {
    sql.value = example.sql
    loadedName.value = null
    void refreshParams(sql.value)
  }

  function loadSaved(query: SavedQuery) {
    sql.value = query.sql
    loadedName.value = query.name
    for (const param of query.params) values[param.name] = param.value ?? ''
    void refreshParams(sql.value)
  }

  function askSave() {
    if (sql.value.trim() === '') {
      status.value = { text: 'Write a query to save first.', error: true }
      return
    }
    prompt.value = { kind: 'save', name: loadedName.value ?? '' }
  }

  /**
   * Saves the query under `name` (trimmed). Saving over another saved query
   * than the loaded one asks first (`replace "<name>"? y/n`); `replace`
   * is the yes.
   */
  async function save(name: string, replace = false) {
    const trimmed = name.trim()
    const taken = saved.value.some((query) => query.name === trimmed)
    if (!replace && taken && trimmed !== loadedName.value) {
      prompt.value = { kind: 'replace', name: trimmed }
      return
    }
    const text = sql.value
    const params = paramValues(await refreshParams(text, false, true), values)
    const answer = await commands.consoleSavedPut({ name: trimmed, sql: text, params, updatedAt: 0 })
    if (answer.status === 'ok') {
      saved.value = answer.data
      loadedName.value = trimmed
      prompt.value = null
      status.value = { text: `Saved "${trimmed}"`, error: false }
    } else {
      status.value = { text: answer.error, error: true }
    }
  }

  function askDelete(name: string) {
    prompt.value = { kind: 'delete', name }
  }

  async function deleteSaved(name: string) {
    prompt.value = null
    const answer = await commands.consoleSavedDelete(name)
    if (answer.status === 'ok') {
      saved.value = answer.data
      if (loadedName.value === name) loadedName.value = null
      status.value = { text: `Deleted "${name}"`, error: false }
    } else {
      status.value = { text: answer.error, error: true }
    }
  }

  function askExport() {
    if (busy.value) return
    if (sql.value.trim() === '') {
      status.value = { text: 'Write a query to export first.', error: true }
      return
    }
    prompt.value = { kind: 'export' }
  }

  async function exportAs(format: ExportFormat) {
    prompt.value = null
    const text = sql.value
    if (busy.value) return
    busy.value = 'export'
    status.value = null
    try {
      const names = await refreshParams(text)
      const answer = await commands.consoleExport(text, paramValues(names, values), format)
      if (answer.status !== 'ok') status.value = { text: answer.error, error: true }
      else if (answer.data !== null) {
        const rows = answer.data
        status.value = { text: `Exported ${rows.toLocaleString('en-US')} ${rows === 1 ? 'row' : 'rows'}`, error: false }
      }
    } finally {
      busy.value = null
    }
  }

  async function openFolder() {
    const answer = await commands.openDatabaseFolder()
    if (answer.status !== 'ok') status.value = { text: answer.error, error: true }
  }

  const views = computed(() => schema.value.filter((object) => !object.raw))
  const rawTables = computed(() => schema.value.filter((object) => object.raw))

  return {
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
    loadSchema,
    loadSavedList,
    setSql,
    setValue,
    run,
    cancel,
    runObject,
    loadExample,
    loadSaved,
    askSave,
    save,
    askDelete,
    deleteSaved,
    askExport,
    exportAs,
    openFolder,
  }
}
