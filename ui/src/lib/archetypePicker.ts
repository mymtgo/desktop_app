import type { ArchetypeCatalogOption, ArchetypePickerOption } from '@/bindings'

/**
 * The `archetypes.format` key for a format code or label (0.x
 * `archetypeFormatKey`, `MtgoFormat::key`): a constructed code's leading C
 * (before a capital) is dropped and the rest lowercased.
 */
export function archetypeFormatKey(format: string): string {
  const raw = /^C[A-Z]/.test(format) ? format.slice(1) : format
  return raw.toLowerCase()
}

/**
 * What a picker lists (0.x `useArchetypeSplit`): the fallbacks (Homebrew and
 * Rogue fit every format, never searched) and the other archetypes of the
 * format (any format when null) whose name holds the search, any case.
 */
export function splitArchetypes(
  archetypes: ArchetypePickerOption[],
  format: string | null,
  search: string,
): { fallbacks: ArchetypePickerOption[]; regular: ArchetypePickerOption[] } {
  const key = format ? archetypeFormatKey(format) : null
  const term = search.trim().toLowerCase()
  const fallbacks = archetypes.filter((archetype) => archetype.isFallback)
  const regular = archetypes.filter(
    (archetype) =>
      !archetype.isFallback &&
      (key === null || archetype.format === key) &&
      (term === '' || archetype.name.toLowerCase().includes(term)),
  )
  return { fallbacks, regular }
}

/** One row of the picker's list: a held archetype (`id`) or a server one v1 has not met (`entry`). */
export interface PickerRow {
  key: string
  name: string
  colors: string[]
  id: number | null
  entry: ArchetypeCatalogOption | null
  /** The player's own archetype, badged "Custom" beside MyMTGO's of the same name. */
  isCustom: boolean
}

/**
 * The picker's regular rows (#489): the held ones `splitArchetypes` kept and
 * the server's archetypes v1 has not met whose name holds the search, one
 * list by name. Without a catalog (offline, still loading) the held ones.
 */
export function pickerRows(
  regular: ArchetypePickerOption[],
  catalog: ArchetypeCatalogOption[] | undefined,
  search: string,
): PickerRow[] {
  const term = search.trim().toLowerCase()
  const rows: PickerRow[] = regular.map((archetype) => ({
    key: `held:${archetype.id}`,
    name: archetype.name,
    colors: archetype.colors,
    id: archetype.id,
    entry: null,
    isCustom: archetype.isCustom,
  }))
  for (const entry of catalog ?? []) {
    if (term !== '' && !entry.name.toLowerCase().includes(term)) continue
    rows.push({ key: `server:${entry.uuid}`, name: entry.name, colors: entry.colors, id: null, entry, isCustom: false })
  }
  return rows.sort((a, b) => a.name.localeCompare(b.name))
}

/** The Set Archetype dialog's line (0.x `SetArchetypeDialog`): `count` matches in bulk, or null for one. */
export function setArchetypeDescription(count: number | null): string {
  if (count === null) return 'Search and select an archetype for this opponent.'
  return `Set archetype for ${count} selected ${count === 1 ? 'match' : 'matches'}.`
}

/** The format a bulk set offers (0.x `openBulkSetArchetype`): the selection's one format, else null. */
export function bulkFormat(formats: (string | null)[]): string | null {
  const distinct = new Set(formats)
  return distinct.size === 1 ? [...distinct][0] : null
}
