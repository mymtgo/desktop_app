import { onScopeDispose, ref, watch, type Ref } from 'vue'
import { commands } from '@/bindings'
import { unwrap } from '@/lib/commands'
import { PickNoteSaver, refreshedNote, type NoteState } from '@/lib/limited'

/** The pick a note editor points at. */
export type PickNoteTarget = {
  draftToken: string
  ordinal: number
  note: string | null
  savedAtMs: number | null
}

type Active = { draftToken: string; saver: PickNoteSaver }

/**
 * A pick note's autosave (0.x `PickNoteEditor` and `useDraftNoteAutosave`),
 * shared by the draft review's editor and the draft notes window: saved
 * 600 ms after the last keystroke (`PickNoteSaver`), and at once on blur
 * (`flush`), on a pick or draft change and when the editor goes away.
 *
 * The target is read through a getter so the caller can point it at a pinned
 * past pick as easily as at the live one, or at nothing (`null`). A save is
 * bound to the draft and pick it was typed for, so one that lands after a
 * change never writes or marks the wrong pick. A pushed or refetched note
 * replaces the text only while nothing typed is unsaved (`refreshedNote`).
 */
export function usePickNoteAutosave(target: () => PickNoteTarget | null): {
  text: Ref<string>
  state: Ref<NoteState>
  /** The text changed (the box's `input`). */
  input: () => void
  /** Save unsaved text now (the box's `blur`). */
  flush: () => void
} {
  const text = ref('')
  const state = ref<NoteState>({ status: 'idle', savedAtMs: null })
  let active: Active | null = null

  function start(pick: PickNoteTarget): void {
    const draftToken = pick.draftToken
    const saver: PickNoteSaver = new PickNoteSaver({
      ordinal: pick.ordinal,
      savedAtMs: pick.savedAtMs,
      save: async (ordinal, note) => {
        unwrap(await commands.setPickNote(draftToken, ordinal, note))
      },
      now: () => Date.now(),
      onChange: (next) => {
        // A save of a draft left behind no longer speaks for the box.
        if (active?.saver === saver) state.value = next
      },
    })
    active = { draftToken, saver }
    text.value = pick.note ?? ''
    state.value = saver.state
  }

  function follow(pick: PickNoteTarget | null): void {
    if (pick === null) {
      active?.saver.flush()
      active = null
      text.value = ''
      state.value = { status: 'idle', savedAtMs: null }
      return
    }
    if (active === null || active.draftToken !== pick.draftToken) {
      active?.saver.flush()
      start(pick)
      return
    }
    const next = refreshedNote(text.value, active.saver.state, { note: pick.note, savedAtMs: pick.savedAtMs })
    if (next !== null) {
      text.value = next
      active.saver.adopt(pick.savedAtMs)
    }
  }

  follow(target())

  watch(
    () => {
      const pick = target()
      return pick === null ? null : ([pick.draftToken, pick.ordinal, pick.note, pick.savedAtMs] as const)
    },
    (next, previous) => {
      const pick = target()
      if (pick !== null && active !== null && next !== null && previous != null) {
        const [draftToken, ordinal] = next
        if (draftToken === active.draftToken && ordinal !== previous[1]) {
          active.saver.switchTo(ordinal, pick.savedAtMs)
          text.value = pick.note ?? ''
          return
        }
      }
      follow(pick)
    },
  )

  function flush(): void {
    active?.saver.flush()
  }

  onScopeDispose(flush)

  return {
    text,
    state,
    input: () => active?.saver.input(text.value),
    flush,
  }
}
