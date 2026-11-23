import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'

const setPickNote = vi.fn()

vi.mock('@/bindings', () => ({ commands: { setPickNote } }))

const { usePickNoteAutosave } = await import('./usePickNoteAutosave')
type Target = Parameters<typeof usePickNoteAutosave>[0] extends () => infer T ? T : never

function target(overrides: Partial<NonNullable<Target>> = {}): NonNullable<Target> {
  return { draftToken: 'q-draft-1', ordinal: 1, note: null, savedAtMs: null, ...overrides }
}

/** The composable inside a scope, as a component's setup runs it. */
function autosave(initial: Target) {
  const current = ref<Target>(initial)
  const scope = effectScope()
  const editor = scope.run(() => usePickNoteAutosave(() => current.value))!
  const type = (text: string) => {
    editor.text.value = text
    editor.input()
  }
  return { current, scope, editor, type }
}

describe('usePickNoteAutosave', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    setPickNote.mockReset()
    setPickNote.mockResolvedValue({ status: 'ok', data: null })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('starts from the pick note and saves 600 ms after the last keystroke', async () => {
    const { editor, type } = autosave(target({ note: 'Qold', savedAtMs: 7 }))
    expect(editor.text.value).toBe('Qold')
    expect(editor.state.value).toEqual({ status: 'idle', savedAtMs: 7 })
    type('Qnew')
    expect(editor.state.value.status).toBe('dirty')
    vi.advanceTimersByTime(599)
    expect(setPickNote).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(setPickNote).toHaveBeenCalledWith('q-draft-1', 1, 'Qnew')
    await vi.runAllTimersAsync()
    expect(editor.state.value.status).toBe('saved')
  })

  it('flushes on blur and when the editor goes away', () => {
    const { editor, scope, type } = autosave(target())
    type('Qblur')
    editor.flush()
    expect(setPickNote).toHaveBeenCalledTimes(1)
    type('Qleaving')
    scope.stop()
    expect(setPickNote).toHaveBeenLastCalledWith('q-draft-1', 1, 'Qleaving')
  })

  it('saves to the old pick on a pick change, then shows the new pick', async () => {
    const { current, editor, type } = autosave(target())
    type('Qfirst')
    current.value = target({ ordinal: 2, note: 'Qsecond', savedAtMs: 9 })
    await nextTick()
    expect(setPickNote).toHaveBeenCalledWith('q-draft-1', 1, 'Qfirst')
    expect(editor.text.value).toBe('Qsecond')
    expect(editor.state.value).toEqual({ status: 'idle', savedAtMs: 9 })
  })

  it('saves to the old draft when a new draft starts', async () => {
    const { current, editor, type } = autosave(target())
    type('Qlast pick')
    current.value = target({ draftToken: 'q-draft-2' })
    await nextTick()
    expect(setPickNote).toHaveBeenCalledWith('q-draft-1', 1, 'Qlast pick')
    expect(editor.text.value).toBe('')
    type('Qfirst pick')
    vi.advanceTimersByTime(600)
    expect(setPickNote).toHaveBeenLastCalledWith('q-draft-2', 1, 'Qfirst pick')
  })

  it('takes a pushed note when nothing typed is unsaved, never over typing', async () => {
    const { current, editor, type } = autosave(target())
    current.value = target({ note: 'Qelsewhere', savedAtMs: 5 })
    await nextTick()
    expect(editor.text.value).toBe('Qelsewhere')
    type('Qtyping')
    current.value = target({ note: 'Qpushed', savedAtMs: 6 })
    await nextTick()
    expect(editor.text.value).toBe('Qtyping')
  })

  it('keeps a note it saved over an older pushed one', async () => {
    vi.setSystemTime(100)
    const { current, editor, type } = autosave(target({ note: 'Qold', savedAtMs: 50 }))
    type('Qnew')
    editor.flush()
    await vi.runAllTimersAsync()
    expect(editor.state.value.status).toBe('saved')
    current.value = target({ note: 'Qolder', savedAtMs: 60 })
    await nextTick()
    expect(editor.text.value).toBe('Qnew')
  })

  it('has nothing to edit without a pick, and saves what was typed first', async () => {
    const { current, editor, type } = autosave(null)
    expect(editor.text.value).toBe('')
    type('Qignored')
    vi.advanceTimersByTime(600)
    expect(setPickNote).not.toHaveBeenCalled()

    current.value = target()
    await nextTick()
    type('Qkept')
    current.value = null
    await nextTick()
    expect(setPickNote).toHaveBeenCalledWith('q-draft-1', 1, 'Qkept')
    expect(editor.text.value).toBe('')
  })

  it('shows not saved when the save is refused', async () => {
    setPickNote.mockResolvedValue({ status: 'error', error: { kind: 'refused', message: 'This pick no longer exists.' } })
    const { editor, type } = autosave(target())
    type('Q')
    editor.flush()
    await vi.runAllTimersAsync()
    expect(editor.state.value.status).toBe('error')
  })
})
