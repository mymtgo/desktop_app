import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, ref } from 'vue'

const setOverlayDeckLabel = vi.fn()

vi.mock('@/bindings', () => ({ commands: { setOverlayDeckLabel } }))

const { useOverlayDeckLabel, LABEL_SAVE_DELAY_MS } = await import('./useOverlayDeckLabel')

/** The composable inside a scope, as the card's setup runs it. */
function labelFor(deckId: number | null, initial: string | null) {
  const id = ref<number | null>(deckId)
  const pushed = ref<string | null>(initial)
  const scope = effectScope()
  const editor = scope.run(() => useOverlayDeckLabel(id, pushed))!
  return { id, pushed, scope, editor }
}

describe('useOverlayDeckLabel', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    setOverlayDeckLabel.mockReset()
    setOverlayDeckLabel.mockResolvedValue({ status: 'ok', data: null })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('starts from the pushed label', () => {
    expect(labelFor(3, 'Qlabel').editor.label.value).toBe('Qlabel')
    expect(labelFor(3, null).editor.label.value).toBe('')
    expect(labelFor(null, 'Qlabel').editor.label.value).toBe('')
  })

  it('saves 500 ms after the last keystroke, a blank label as none', async () => {
    const { editor } = labelFor(3, null)
    expect(LABEL_SAVE_DELAY_MS).toBe(500)
    editor.setLabel('Q')
    vi.advanceTimersByTime(300)
    editor.setLabel('Qs')
    vi.advanceTimersByTime(499)
    expect(setOverlayDeckLabel).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    await vi.runAllTimersAsync()
    expect(setOverlayDeckLabel).toHaveBeenCalledTimes(1)
    expect(setOverlayDeckLabel).toHaveBeenCalledWith(3, 'Qs')
    editor.setLabel('   ')
    await vi.runAllTimersAsync()
    expect(setOverlayDeckLabel).toHaveBeenLastCalledWith(3, null)
  })

  it('never overwrites unsaved typing with a push', async () => {
    const { pushed, editor } = labelFor(3, 'Qold')
    editor.setLabel('Qtyping')
    pushed.value = 'Qolder push'
    await nextTick()
    expect(editor.label.value).toBe('Qtyping')
  })

  it('waits for its own save to echo back before taking pushes again', async () => {
    let finish: (value: unknown) => void = () => {}
    setOverlayDeckLabel.mockImplementation(() => new Promise((resolve) => (finish = resolve)))
    const { pushed, editor } = labelFor(3, 'Qold')
    editor.setLabel('  Qnew  ')
    vi.advanceTimersByTime(500)
    // A push read before the save landed: still the typed text.
    pushed.value = 'Qold'
    await nextTick()
    expect(editor.label.value).toBe('  Qnew  ')
    finish({ status: 'ok', data: null })
    await vi.runAllTimersAsync()
    // Older than the save: ignored.
    pushed.value = 'Qstale'
    await nextTick()
    expect(editor.label.value).toBe('  Qnew  ')
    // The echo (trimmed, as stored), then later pushes apply.
    pushed.value = 'Qnew'
    await nextTick()
    expect(editor.label.value).toBe('Qnew')
    pushed.value = 'Qrenamed elsewhere'
    await nextTick()
    expect(editor.label.value).toBe('Qrenamed elsewhere')
  })

  it('expects the echo cut to 40 characters', async () => {
    const { pushed, editor } = labelFor(3, null)
    const long = 'Q'.repeat(45)
    editor.setLabel(long)
    await vi.runAllTimersAsync()
    pushed.value = 'Q'.repeat(40)
    await nextTick()
    expect(editor.label.value).toBe('Q'.repeat(40))
  })

  it('takes pushes again after a refused save', async () => {
    setOverlayDeckLabel.mockResolvedValue({ status: 'error', error: { type: 'Refused', data: 'x' } })
    const { pushed, editor } = labelFor(3, 'Qold')
    editor.setLabel('Qnew')
    await vi.runAllTimersAsync()
    pushed.value = 'Qserver'
    await nextTick()
    expect(editor.label.value).toBe('Qserver')
  })

  it('starts over for another deck and saves nothing without one', async () => {
    const { id, pushed, editor } = labelFor(3, 'Qone')
    editor.setLabel('Qtyped')
    pushed.value = 'Qtwo'
    id.value = 4
    await nextTick()
    expect(editor.label.value).toBe('Qtwo')
    id.value = null
    await nextTick()
    expect(editor.label.value).toBe('')
    setOverlayDeckLabel.mockClear()
    editor.setLabel('Qnowhere')
    await vi.runAllTimersAsync()
    expect(setOverlayDeckLabel).not.toHaveBeenCalledWith(null, expect.anything())
    expect(editor.label.value).toBe('Qnowhere')
  })
})
