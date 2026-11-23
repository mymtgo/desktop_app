import type { ConsoleParam } from '@/bindings'

/** How a named parameter is entered: a picker of names, or free text. */
export type PickerKind = 'archetype' | 'deck' | 'text'

/** A parameter whose name ends in `archetype` or `deck` (any case) gets a picker. */
export function pickerKind(name: string): PickerKind {
  const lower = name.toLowerCase()
  if (lower.endsWith('archetype')) return 'archetype'
  if (lower.endsWith('deck')) return 'deck'
  return 'text'
}

/** The query that lists a picker's choices, each name once; a text field has none. */
export function pickerSql(kind: PickerKind): string | null {
  if (kind === 'archetype') return "SELECT DISTINCT name FROM v_archetypes WHERE name <> '' ORDER BY name"
  if (kind === 'deck') return "SELECT DISTINCT name FROM v_decks WHERE name <> '' ORDER BY name"
  return null
}

/** The values sent with a run: one per listed name, an empty field as SQL NULL. */
export function paramValues(names: string[], values: Record<string, string>): ConsoleParam[] {
  return names.map((name) => {
    const value = values[name] ?? ''
    return { name, value: value === '' ? null : value }
  })
}
