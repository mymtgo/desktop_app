/** What a key press does in the console window. */
export type ConsoleAction = 'run' | 'cancel' | 'nextPane' | 'prevPane' | 'save' | 'export' | 'chart' | 'folder'

export type ConsoleKey = { key: string; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean; altKey?: boolean }

const LETTERS: Record<string, ConsoleAction> = { s: 'save', e: 'export', c: 'chart', o: 'folder' }

/**
 * The action for a key press. `inEditor` is true while a text field has
 * focus (the query editor, a parameter, a prompt): there Tab and single
 * letters type as usual, and only Ctrl+Tab moves between panes.
 */
export function actionFor(event: ConsoleKey, inEditor: boolean): ConsoleAction | null {
  const command = event.ctrlKey || event.metaKey
  if (event.key === 'Enter') return command ? 'run' : null
  if (event.key === 'Escape') return 'cancel'
  if (event.key === 'Tab') {
    if (inEditor && !event.ctrlKey) return null
    return event.shiftKey ? 'prevPane' : 'nextPane'
  }
  if (inEditor || command || event.altKey) return null
  return LETTERS[event.key.toLowerCase()] ?? null
}
