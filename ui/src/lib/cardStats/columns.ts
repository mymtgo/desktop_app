/**
 * The Card Stats table's columns, their picker groups and the visibility
 * stored in `localStorage` (0.x `cardStatsColumns.ts`).
 */
import type { CardStatView } from '@/bindings'
import { readStoredJson, writeStoredJson, type KeyValueStorage } from '@/lib/browserStorage'
import type { Perspective } from './shrinkage'

/** Casting method columns (hidden by default); `statField` is the total each reads and sorts on. */
export const CASTING_METHOD_COLUMNS = [
  { key: 'warp', label: 'Warp', statField: 'totalWarp' },
  { key: 'freeCast', label: 'Free Cast', statField: 'totalFreeCast' },
  { key: 'bargained', label: 'Bargain', statField: 'totalBargained' },
  { key: 'dashed', label: 'Dash', statField: 'totalDashed' },
  { key: 'bestowed', label: 'Bestow', statField: 'totalBestowed' },
  { key: 'replicated', label: 'Replicate', statField: 'totalReplicated' },
  { key: 'spectacle', label: 'Spectacle', statField: 'totalSpectacle' },
  { key: 'rebound', label: 'Rebound', statField: 'totalRebound' },
  { key: 'escaped', label: 'Escape', statField: 'totalEscaped' },
  { key: 'ninjutsu', label: 'Ninjutsu', statField: 'totalNinjutsu' },
  { key: 'suspended', label: 'Suspend', statField: 'totalSuspended' },
  { key: 'buyback', label: 'Buyback', statField: 'totalBuyback' },
  { key: 'disturb', label: 'Disturb', statField: 'totalDisturb' },
  { key: 'foretold', label: 'Foretell', statField: 'totalForetold' },
  { key: 'retraced', label: 'Retrace', statField: 'totalRetraced' },
  { key: 'mayhem', label: 'Mayhem', statField: 'totalMayhem' },
  { key: 'miracle', label: 'Miracle', statField: 'totalMiracle' },
  { key: 'gifted', label: 'Gift', statField: 'totalGifted' },
  { key: 'casualty', label: 'Casualty', statField: 'totalCasualty' },
] as const satisfies readonly { key: string; label: string; statField: keyof CardStatView }[]

export type CastingMethodColumnKey = (typeof CASTING_METHOD_COLUMNS)[number]['key']

/** Every column in table order. */
export const CARD_STATS_COLUMNS = [
  { key: 'type', label: 'Type' },
  { key: 'sb', label: 'SB' },
  { key: 'keptPct', label: 'Kept %' },
  { key: 'keptWinPct', label: 'Kept Win %' },
  { key: 'castPct', label: 'Cast %' },
  { key: 'castWinPct', label: 'Cast Win %' },
  { key: 'playedPct', label: 'Played %' },
  { key: 'kicked', label: 'Kicked' },
  { key: 'activated', label: 'Activated' },
  ...CASTING_METHOD_COLUMNS.map(({ key, label }) => ({ key, label })),
  { key: 'pregamePct', label: 'Pregame %' },
  { key: 'pregameWinPct', label: 'Pregame Win %' },
  { key: 'seenPct', label: 'Seen %' },
  { key: 'seenWinPct', label: 'Seen Win %' },
  { key: 'sbOutPct', label: 'SB Out %' },
  { key: 'sbInPct', label: 'SB In %' },
  { key: 'games', label: 'Games' },
] as const

export type CardStatsColumnKey = (typeof CARD_STATS_COLUMNS)[number]['key']

/** The picker's groups; display only, the table keeps `CARD_STATS_COLUMNS` order. */
export const CARD_STATS_COLUMN_GROUPS: readonly { label: string; keys: readonly CardStatsColumnKey[] }[] = [
  { label: 'Card info', keys: ['type', 'sb'] },
  {
    label: 'Game metrics',
    keys: [
      'keptPct',
      'keptWinPct',
      'castPct',
      'castWinPct',
      'playedPct',
      'pregamePct',
      'pregameWinPct',
      'seenPct',
      'seenWinPct',
      'sbOutPct',
      'sbInPct',
      'games',
    ],
  },
  { label: 'Casting costs', keys: ['kicked', 'activated', ...CASTING_METHOD_COLUMNS.map((col) => col.key)] },
]

export type Visibility = Record<CardStatsColumnKey, boolean>

/** Columns that only make sense for the local player's deck: the opponent has no deck quantity, hand or sideboard. */
export const LOCAL_ONLY_COLUMNS: readonly CardStatsColumnKey[] = ['sb', 'keptPct', 'keptWinPct', 'sbOutPct', 'sbInPct']

const CASTING_KEYS: readonly string[] = CASTING_METHOD_COLUMNS.map((col) => col.key)

/** Every column shown except the casting methods. */
export const DEFAULT_VISIBILITY: Visibility = Object.fromEntries(
  CARD_STATS_COLUMNS.map((col) => [col.key, !CASTING_KEYS.includes(col.key)]),
) as Visibility

export const VISIBILITY_STORAGE_KEY = 'cardStatsVisibleColumns'

/** What is stored merged over the defaults: unknown keys and non-booleans are ignored. */
export function loadVisibility(storage: KeyValueStorage): Visibility {
  const parsed = readStoredJson(storage, VISIBILITY_STORAGE_KEY)
  if (typeof parsed !== 'object' || parsed === null) return { ...DEFAULT_VISIBILITY }
  const record = parsed as Record<string, unknown>
  const merged: Visibility = { ...DEFAULT_VISIBILITY }
  for (const col of CARD_STATS_COLUMNS) {
    const value = record[col.key]
    if (typeof value === 'boolean') merged[col.key] = value
  }
  return merged
}

export function saveVisibility(storage: KeyValueStorage, visibility: Visibility): void {
  writeStoredJson(storage, VISIBILITY_STORAGE_KEY, visibility)
}

/** Bo1 has no sideboarding: these columns have nothing to say. */
export const SIDEBOARD_COLUMNS: readonly CardStatsColumnKey[] = ['sb', 'sbOutPct', 'sbInPct']

/** The columns hidden for a perspective and Bo1 mode. */
function hiddenColumns(perspective: Perspective, bo1: boolean): CardStatsColumnKey[] {
  const hidden: CardStatsColumnKey[] = []
  if (perspective === 'theirs') hidden.push(...LOCAL_ONLY_COLUMNS)
  if (bo1) hidden.push(...SIDEBOARD_COLUMNS)
  return hidden
}

/**
 * The visibility shown: their cards hide the local-only columns and Bo1 mode
 * hides the sideboard columns, whatever was chosen.
 */
export function effectiveVisibility(visibility: Visibility, perspective: Perspective, bo1 = false): Visibility {
  const hidden = hiddenColumns(perspective, bo1)
  if (hidden.length === 0) return visibility
  const overridden = { ...visibility }
  for (const key of hidden) overridden[key] = false
  return overridden
}

/** The columns the picker offers. */
export function configurableColumns(perspective: Perspective, bo1 = false) {
  const hidden = hiddenColumns(perspective, bo1)
  if (hidden.length === 0) return CARD_STATS_COLUMNS
  return CARD_STATS_COLUMNS.filter((col) => !hidden.includes(col.key))
}

/** The picker's groups, each alphabetical by label; empty groups left out. */
export function pickerGroups(perspective: Perspective, bo1 = false) {
  const columns = configurableColumns(perspective, bo1)
  return CARD_STATS_COLUMN_GROUPS.map((group) => ({
    label: group.label,
    columns: columns.filter((col) => group.keys.includes(col.key)).sort((a, b) => a.label.localeCompare(b.label)),
  })).filter((group) => group.columns.length > 0)
}
