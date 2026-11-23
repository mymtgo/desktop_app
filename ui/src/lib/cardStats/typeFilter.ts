/**
 * The Card Stats type filter (0.x `CardStatsView.vue`): the eight types and a
 * Sideboard toggle, stored in `localStorage`. A sideboard card follows only
 * the Sideboard toggle.
 */
import { readStoredJson, writeStoredJson, type KeyValueStorage } from '@/lib/browserStorage'

export const TYPE_KEYS = [
  'Creature',
  'Instant',
  'Sorcery',
  'Enchantment',
  'Artifact',
  'Land',
  'Planeswalker',
  'Battle',
] as const

export type TypeKey = (typeof TYPE_KEYS)[number]
export type FilterKey = TypeKey | 'Sideboard'
export type TypeFilters = Record<FilterKey, boolean>

export const FILTER_KEYS: FilterKey[] = [...TYPE_KEYS, 'Sideboard']

export const TYPE_FILTER_STORAGE_KEY = 'cardStatsTypeFilters'

export const ALL_ENABLED = Object.fromEntries(FILTER_KEYS.map((key) => [key, true])) as TypeFilters

/** Precedence for a type line holding several types (0.x: the first match wins). */
const PRECEDENCE: TypeKey[] = ['Creature', 'Planeswalker', 'Battle', 'Instant', 'Sorcery', 'Enchantment', 'Artifact', 'Land']

/** The category of a type line, or null for none recognised (`Other`, never hidden). */
export function typeCategory(raw: string | null): TypeKey | null {
  if (!raw) return null
  return PRECEDENCE.find((type) => raw.includes(type)) ?? null
}

type Filterable = { type: string | null; isSideboard: boolean }

/** What is stored, when it holds every key; else everything on. */
export function loadTypeFilters(storage: KeyValueStorage): TypeFilters {
  // Broken storage: the defaults.
  const parsed = readStoredJson(storage, TYPE_FILTER_STORAGE_KEY)
  if (typeof parsed === 'object' && parsed !== null && FILTER_KEYS.every((key) => key in parsed)) {
    return parsed as TypeFilters
  }
  return { ...ALL_ENABLED }
}

export function saveTypeFilters(storage: KeyValueStorage, filters: TypeFilters): void {
  writeStoredJson(storage, TYPE_FILTER_STORAGE_KEY, filters)
}

/** The toggles worth showing: the types the stats hold, and Sideboard when a card is in it. */
export function presentFilterKeys(stats: readonly Filterable[]): Set<FilterKey> {
  const present = new Set<FilterKey>()
  for (const stat of stats) {
    if (stat.isSideboard) present.add('Sideboard')
    const category = typeCategory(stat.type)
    if (category) present.add(category)
  }
  return present
}

/** A hidden type that is no longer present is switched back on, so it cannot hide cards unseen. Null when nothing changes. */
export function reenableVanished(filters: TypeFilters, present: ReadonlySet<FilterKey>): TypeFilters | null {
  let next: TypeFilters | null = null
  for (const key of FILTER_KEYS) {
    if (!present.has(key) && !filters[key]) {
      next = next ?? { ...filters }
      next[key] = true
    }
  }
  return next
}

/** Whether a card shows: sideboard cards follow Sideboard only, a card with no recognised type always shows. */
export function passesTypeFilter(stat: Filterable, filters: TypeFilters): boolean {
  if (stat.isSideboard) return filters.Sideboard
  const category = typeCategory(stat.type)
  return category === null || filters[category]
}
