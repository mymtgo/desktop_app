import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ConsoleObject, ConsoleParam, ConsoleResult } from '@/bindings'
import { findAll, focusLog, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

type Answer<T> = { status: 'ok'; data: T } | { status: 'error'; error: string }

const state = vi.hoisted(() => ({
  runs: [] as { sql: string; params: ConsoleParam[] }[],
  answer: null as null | ((sql: string) => Answer<ConsoleResult>),
  // Runs while consoleParameters answers, before the answer arrives.
  duringParams: null as null | (() => void),
  puts: [] as { name: string; sql: string; params: ConsoleParam[] }[],
  exports: [] as { sql: string; format: string }[],
  keydown: [] as ((event: unknown) => void)[],
  savedList: [] as { name: string; sql: string; params: ConsoleParam[]; updatedAt: number }[],
  // Answers consoleParameters with an error (the text does not parse).
  paramsFail: false,
}))

const schema: ConsoleObject[] = [
  {
    name: 'v_matches',
    kind: 'view',
    raw: false,
    columns: [{ name: 'format', declType: '', description: 'The format code.' }],
  },
  { name: 'matches', kind: 'table', raw: true, columns: [] },
]

vi.mock('@/bindings', () => ({
  commands: {
    consoleSchema: async () => ({ status: 'ok', data: schema }),
    consoleSavedList: async () => ({ status: 'ok', data: state.savedList }),
    consoleParameters: async (sql: string) => {
      const during = state.duringParams
      state.duringParams = null
      during?.()
      if (state.paramsFail) return { status: 'error', error: 'near "FROM": syntax error' }
      return { status: 'ok', data: [...new Set([...sql.matchAll(/:(\w+)/g)].map((match) => match[1]))] }
    },
    consoleRun: async (sql: string, params: ConsoleParam[]) => {
      state.runs.push({ sql, params })
      if (state.answer) return state.answer(sql)
      return { status: 'ok', data: { columns: [], rows: [], truncated: false, elapsedMs: 0 } }
    },
    consoleCancel: async () => {},
    consoleExport: async (sql: string, _params: ConsoleParam[], format: string) => {
      state.exports.push({ sql, format })
      return { status: 'ok', data: null }
    },
    consoleSavedPut: async (query: { name: string; sql: string; params: ConsoleParam[] }) => {
      state.puts.push(query)
      return { status: 'ok', data: state.savedList }
    },
    consoleSavedDelete: async () => ({ status: 'ok', data: [] }),
    openDatabaseFolder: async () => ({ status: 'ok', data: null }),
  },
}))

const { default: ConsoleRoot } = await import('./ConsoleRoot.vue')

beforeEach(() => {
  state.runs = []
  state.answer = null
  state.duringParams = null
  state.puts = []
  state.exports = []
  state.keydown = []
  state.savedList = []
  state.paramsFail = false
  const listeners = { addEventListener: () => {}, removeEventListener: () => {} }
  vi.stubGlobal('window', {
    addEventListener: (type: string, handler: (event: unknown) => void) => {
      if (type === 'keydown') state.keydown.push(handler)
    },
    removeEventListener: () => {},
    setTimeout,
    clearTimeout,
  })
  vi.stubGlobal('document', { ...listeners, activeElement: null })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function editor(root: FakeNode): FakeNode {
  return findAll(root, (node) => node.tag === 'textarea')[0]
}

function type(mounted: ReturnType<typeof mountFake>, sql: string) {
  ;(editor(mounted.root).props.onInput as (event: unknown) => void)({ target: { value: sql } })
}

function press(mounted: ReturnType<typeof mountFake>, action: string) {
  const button = findAll(mounted.root, (node) => node.tag === 'button' && node.props['data-action'] === action)[0]
  ;(button.props.onClick as () => void)()
}

function key(key: string, tagName: string) {
  const event = { key, ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, target: { tagName }, preventDefault: () => {} }
  for (const handler of state.keydown) handler(event)
}

function nameField(root: FakeNode): FakeNode {
  return findAll(root, (node) => node.tag === 'input' && node.props['aria-label'] === 'Saved query name')[0]
}

async function saveAs(mounted: ReturnType<typeof mountFake>, name: string) {
  press(mounted, 'save')
  await mounted.settle()
  const field = nameField(mounted.root)
  ;(field.props.onInput as (event: unknown) => void)({ target: { value: name } })
  await mounted.settle()
  ;(field.props.onKeydown as (event: unknown) => void)({ key: 'Enter', ctrlKey: false, metaKey: false, preventDefault: () => {} })
  await mounted.settle()
}

function promptBox(root: FakeNode, kind: string): FakeNode | undefined {
  return findAll(root, (node) => node.props['data-prompt'] === kind)[0]
}

async function typeAndRun(mounted: ReturnType<typeof mountFake>, sql: string) {
  await mounted.settle()
  type(mounted, sql)
  await mounted.settle()
  press(mounted, 'run')
  await mounted.settle()
}

describe('the data console', () => {
  it('running shows the columns and a row', async () => {
    state.answer = () => ({
      status: 'ok',
      data: {
        columns: ['format', 'matches', 'note'],
        rows: [[{ kind: 'text', value: 'CMODERN' }, { kind: 'integer', value: 214 }, { kind: 'null' }]],
        truncated: false,
        elapsedMs: 12,
      },
    })
    const mounted = mountFake(ConsoleRoot)
    await typeAndRun(mounted, 'SELECT format, count(*) AS matches FROM v_matches GROUP BY format')
    expect(mounted.warnings).toEqual([])
    expect(state.runs.at(-1)?.sql).toBe('SELECT format, count(*) AS matches FROM v_matches GROUP BY format')
    const headers = findAll(mounted.root, (node) => node.tag === 'th').map(textOf)
    expect(headers).toEqual(['format', 'matches', 'note'])
    const cells = findAll(mounted.root, (node) => node.tag === 'td').map(textOf)
    expect(cells).toEqual(['CMODERN', '214', 'NULL'])
    expect(textOf(mounted.root)).toContain('1 row')
    expect(textOf(mounted.root)).toContain('12 ms')
    mounted.unmount()
  })

  it('an error result shows its message', async () => {
    state.answer = () => ({ status: 'error', error: 'no such column: nope' })
    const mounted = mountFake(ConsoleRoot)
    await typeAndRun(mounted, 'SELECT nope FROM v_matches')
    const errors = findAll(mounted.root, (node) => node.props['data-error'] === 'results')
    expect(errors.map(textOf)).toEqual(['no such column: nope'])
    mounted.unmount()
  })

  it('a query with an archetype parameter shows one picker', async () => {
    state.answer = (sql) =>
      sql.includes('v_archetypes')
        ? {
            status: 'ok',
            data: {
              columns: ['name'],
              rows: [[{ kind: 'text', value: 'Burn' }], [{ kind: 'text', value: 'Tron' }]],
              truncated: false,
              elapsedMs: 1,
            },
          }
        : { status: 'ok', data: { columns: [], rows: [], truncated: false, elapsedMs: 1 } }
    const mounted = mountFake(ConsoleRoot)
    await typeAndRun(mounted, 'SELECT * FROM v_card_games WHERE archetype = :first_archetype')
    const pickers = findAll(mounted.root, (node) => node.tag === 'select')
    expect(pickers).toHaveLength(1)
    expect(pickers[0].props['data-param']).toBe('first_archetype')
    const options = findAll(pickers[0], (node) => node.tag === 'option').map(textOf)
    expect(options).toEqual(['NULL', 'Burn', 'Tron'])
    // The query itself ran with the empty picker sent as null.
    // One control per parameter: no text field beside the picker, and no
    // truncation note when the list was not cut.
    const inputs = findAll(mounted.root, (node) => node.tag === 'input' && node.props['data-param'] === 'first_archetype')
    expect(inputs).toHaveLength(0)
    expect(findAll(mounted.root, (node) => node.props['data-note'] === 'truncated')).toHaveLength(0)
    const ran = state.runs.find((run) => run.sql.includes('v_card_games'))
    expect(ran?.params).toEqual([{ name: 'first_archetype', value: null }])
    mounted.unmount()
  })

  it('a run sends the text it saw, even when typing goes on meanwhile', async () => {
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    type(mounted, 'SELECT * FROM v_matches WHERE format = :format')
    await mounted.settle()
    state.duringParams = () => type(mounted, 'SELECT * FROM v_decks')
    press(mounted, 'run')
    await mounted.settle()
    expect(state.runs.at(-1)).toEqual({
      sql: 'SELECT * FROM v_matches WHERE format = :format',
      params: [{ name: 'format', value: null }],
    })
    mounted.unmount()
  })

  it('saving right after typing keeps the parameters of that text', async () => {
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    // Saved at once: the 400 ms parameter refresh after typing has not run.
    type(mounted, 'SELECT * FROM v_matches WHERE format = :format')
    await mounted.settle()
    press(mounted, 'save')
    await mounted.settle()
    const name = findAll(mounted.root, (node) => node.tag === 'input' && node.props['aria-label'] === 'Saved query name')[0]
    ;(name.props.onInput as (event: unknown) => void)({ target: { value: 'by format' } })
    await mounted.settle()
    ;(name.props.onKeydown as (event: unknown) => void)({ key: 'Enter', ctrlKey: false, metaKey: false, preventDefault: () => {} })
    await mounted.settle()
    expect(state.puts).toEqual([
      expect.objectContaining({
        name: 'by format',
        sql: 'SELECT * FROM v_matches WHERE format = :format',
        params: [{ name: 'format', value: null }],
      }),
    ])
    mounted.unmount()
  })

  it('c shows the result as a chart with one bar per row', async () => {
    state.answer = () => ({
      status: 'ok',
      data: {
        columns: ['format', 'matches'],
        rows: [
          [{ kind: 'text', value: 'CMODERN' }, { kind: 'integer', value: 214 }],
          [{ kind: 'text', value: 'CPAUPER' }, { kind: 'integer', value: 31 }],
          [{ kind: 'text', value: 'CLEGACY' }, { kind: 'integer', value: 9 }],
        ],
        truncated: true,
        elapsedMs: 3,
      },
    })
    const mounted = mountFake(ConsoleRoot)
    await typeAndRun(mounted, 'SELECT format, count(*) AS matches FROM v_matches GROUP BY format')
    expect(findAll(mounted.root, (node) => node.tag === 'svg')).toHaveLength(0)
    key('c', 'BUTTON')
    await mounted.settle()
    expect(mounted.warnings).toEqual([])
    expect(findAll(mounted.root, (node) => node.tag === 'table')).toHaveLength(0)
    const svgs = findAll(mounted.root, (node) => node.tag === 'svg')
    expect(svgs.map((node) => node.props['aria-label'])).toEqual(['matches by format'])
    const bars = findAll(mounted.root, (node) => node.tag === 'rect' && node.props['data-bar'] !== undefined)
    expect(bars).toHaveLength(3)
    const pickers = findAll(mounted.root, (node) => node.tag === 'select').map((node) => node.props['aria-label'])
    expect(pickers).toEqual(['Chart X column', 'Chart Y column', 'Chart kind'])
    const title = findAll(mounted.root, (node) => node.props.class === 'tui-chart-title').map(textOf)
    expect(title).toEqual(['matches by formatfirst 3 rows'])
    // The pane title's Grid toggle goes back to the grid.
    const grid = findAll(mounted.root, (node) => node.tag === 'button' && node.props['data-view'] === 'grid')[0]
    ;(grid.props.onClick as () => void)()
    await mounted.settle()
    expect(findAll(mounted.root, (node) => node.tag === 'svg')).toHaveLength(0)
    mounted.unmount()
  })

  it('the chart says so when no column is numeric', async () => {
    state.answer = () => ({
      status: 'ok',
      data: { columns: ['format'], rows: [[{ kind: 'text', value: 'CMODERN' }]], truncated: false, elapsedMs: 1 },
    })
    const mounted = mountFake(ConsoleRoot)
    await typeAndRun(mounted, 'SELECT format FROM v_matches')
    press(mounted, 'chart')
    await mounted.settle()
    expect(findAll(mounted.root, (node) => node.tag === 'svg')).toHaveLength(0)
    expect(textOf(mounted.root)).toContain('No numeric column to chart.')
    mounted.unmount()
  })

  it('the export prompt letters type as usual in a text field', async () => {
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    type(mounted, 'SELECT * FROM v_matches')
    await mounted.settle()
    press(mounted, 'export')
    await mounted.settle()
    key('c', 'TEXTAREA')
    key('j', 'INPUT')
    await mounted.settle()
    expect(state.exports).toEqual([])
    key('c', 'BUTTON')
    await mounted.settle()
    expect(state.exports).toEqual([{ sql: 'SELECT * FROM v_matches', format: 'csv' }])
    mounted.unmount()
  })

  it('the export prompt takes focus, so its letters answer it', async () => {
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    type(mounted, 'SELECT * FROM v_matches')
    await mounted.settle()
    press(mounted, 'export')
    await mounted.settle()
    const box = promptBox(mounted.root, 'export')
    expect(box).toBeDefined()
    expect(box?.props.tabindex).toBe('-1')
    expect(focusLog.at(-1)).toBe(box)
    mounted.unmount()
  })

  it('the delete prompt takes focus', async () => {
    state.savedList = [{ name: 'Win rate', sql: 'SELECT 1', params: [], updatedAt: 1 }]
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    const remove = findAll(mounted.root, (node) => node.tag === 'button' && hasDeleteClass(node))[0]
    ;(remove.props.onClick as () => void)()
    await mounted.settle()
    const box = promptBox(mounted.root, 'delete')
    expect(box).toBeDefined()
    expect(focusLog.at(-1)).toBe(box)
    mounted.unmount()
  })

  it('the save prompt keeps focus in its name field', async () => {
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    type(mounted, 'SELECT 1')
    await mounted.settle()
    press(mounted, 'save')
    await mounted.settle()
    expect(focusLog.at(-1)).toBe(nameField(mounted.root))
    mounted.unmount()
  })

  it('saving over another saved query asks first, and only y saves', async () => {
    state.savedList = [{ name: 'Win rate', sql: 'SELECT 1', params: [], updatedAt: 1 }]
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    type(mounted, 'SELECT 2')
    await mounted.settle()
    await saveAs(mounted, '  Win rate ')
    expect(state.puts).toEqual([])
    const box = promptBox(mounted.root, 'replace')
    expect(box && textOf(box)).toContain('replace "Win rate"?')
    expect(focusLog.at(-1)).toBe(box)
    key('n', 'DIV')
    await mounted.settle()
    expect(promptBox(mounted.root, 'replace')).toBeUndefined()
    expect(state.puts).toEqual([])

    await saveAs(mounted, 'Win rate')
    key('y', 'DIV')
    await mounted.settle()
    expect(state.puts).toEqual([expect.objectContaining({ name: 'Win rate', sql: 'SELECT 2' })])
    expect(promptBox(mounted.root, 'replace')).toBeUndefined()
    mounted.unmount()
  })

  it('saving the loaded query under its own name does not ask', async () => {
    state.savedList = [{ name: 'Win rate', sql: 'SELECT 1', params: [], updatedAt: 1 }]
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    const load = findAll(mounted.root, (node) => node.props['data-saved'] === 'Win rate')[0]
    ;(load.props.onClick as () => void)()
    await mounted.settle()
    await saveAs(mounted, 'Win rate')
    expect(promptBox(mounted.root, 'replace')).toBeUndefined()
    expect(state.puts).toEqual([expect.objectContaining({ name: 'Win rate', sql: 'SELECT 1' })])
    mounted.unmount()
  })

  it('a save of a query that does not parse stores no parameter names', async () => {
    const mounted = mountFake(ConsoleRoot)
    await mounted.settle()
    type(mounted, 'SELECT * FROM v_matches WHERE format = :format')
    await mounted.settle()
    // The fields now list :format; the next text does not parse.
    press(mounted, 'run')
    await mounted.settle()
    state.paramsFail = true
    type(mounted, 'SELECT :other FROM')
    await mounted.settle()
    await saveAs(mounted, 'broken')
    expect(state.puts).toEqual([expect.objectContaining({ name: 'broken', params: [] })])
    mounted.unmount()
  })

  it('a picker cut at the run cap says so', async () => {
    state.answer = (sql) =>
      sql.includes('v_decks')
        ? {
            status: 'ok',
            data: { columns: ['name'], rows: [[{ kind: 'text', value: 'Burn' }]], truncated: true, elapsedMs: 1 },
          }
        : { status: 'ok', data: { columns: [], rows: [], truncated: false, elapsedMs: 1 } }
    const mounted = mountFake(ConsoleRoot)
    await typeAndRun(mounted, 'SELECT * FROM v_matches WHERE deck = :deck')
    const notes = findAll(mounted.root, (node) => node.props['data-note'] === 'truncated').map(textOf)
    expect(notes).toEqual(['first 1,000 names'])
    expect(findAll(mounted.root, (node) => node.tag === 'select' && node.props['data-param'] === 'deck')).toHaveLength(1)
    expect(findAll(mounted.root, (node) => node.tag === 'input' && node.props['data-param'] === 'deck')).toHaveLength(0)
    mounted.unmount()
  })
})

function hasDeleteClass(node: FakeNode): boolean {
  return String(node.props.class ?? '').includes('tui-delete')
}
