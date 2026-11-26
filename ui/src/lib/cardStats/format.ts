/**
 * Display rules for the card stats cells and tiles (0.x `cardStatFormat.ts`)
 * and the tab's small messages.
 */
import type { Perspective, ShrinkKey } from './shrinkage'

/** The trust setting's default, and what "Reset to 50" sets. */
export const TRUST_DEFAULT = 50

/** The trust setting's largest value (0.x `UpdateTrustSettingController`). */
export const TRUST_HARD_MAX = 100_000

/** A whole-number percentage, or null without a denominator. */
export function pct(num: number, denom: number): number | null {
  return denom > 0 ? Math.round((num / denom) * 100) : null
}

/** A shrunk win rate (0 to 1) as a whole percentage. */
export function shrunkWinPct(shrunk: Readonly<Record<ShrinkKey, number>>, key: ShrinkKey): number {
  return Math.round(shrunk[key] * 100)
}

/** The deck's baseline win rate from the shown side, as a whole percentage. */
export function baselinePct(prior: number, perspective: Perspective): number {
  return Math.round((perspective === 'theirs' ? 1 - prior : prior) * 100)
}

/** The tooltip behind a win %: the raw numbers, and the baseline it was pulled toward. */
export function rawWinPctLabel(
  rawRates: Readonly<Record<ShrinkKey, number | null>>,
  samples: Readonly<Record<ShrinkKey, number>>,
  key: ShrinkKey,
  trust: number,
  prior: number,
  perspective: Perspective,
): string {
  const raw = rawRates[key]
  const games = samples[key]
  if (raw === null || games === 0) return 'no data'
  const rawLabel = `Raw ${Math.round(raw * 100)}% over ${games} game${games === 1 ? '' : 's'}`
  if (trust <= 0) return rawLabel
  return `${rawLabel} · adjusted toward ${baselinePct(prior, perspective)}% deck baseline`
}

/**
 * Above 55 is green and below 45 red; for their cards a high win rate
 * against you is bad for them, so the colours swap.
 */
export function winRateClass(value: number, perspective: Perspective): string {
  if (perspective === 'theirs') {
    if (value > 55) return 'text-destructive'
    if (value < 45) return 'text-success'
    return ''
  }
  if (value > 55) return 'text-success'
  if (value < 45) return 'text-destructive'
  return ''
}

/** The trust slider's top: `max(200, ceil(games * 2 / 50) * 50)`. */
export function trustMax(games: number): number {
  if (!Number.isFinite(games) || games <= 0) return 200
  return Math.max(200, Math.ceil((games * 2) / 50) * 50)
}

/** A trust value as the setting stores it: whole, 0 to 100,000. */
export function clampTrust(value: number): number {
  if (!Number.isFinite(value)) return TRUST_DEFAULT
  return Math.min(TRUST_HARD_MAX, Math.max(0, Math.round(value)))
}

/** The "Card stats regenerating" toast's text. */
export function regenerateMessage(count: number): string {
  if (count === 0) return 'No matches needed regeneration.'
  return `Queued regeneration for ${count} match${count === 1 ? '' : 'es'}. Stats will refresh shortly.`
}

/** The regenerate failure toast's text, exactly 0.x's (nothing appended). */
export const REGENERATE_FAILED_MESSAGE = 'Could not regenerate card stats.'

/** The SB In marker's tooltip (cards spec ruling 8). */
export function inferredTooltip(games: number): string {
  return `Includes ${games} imported game${games === 1 ? '' : 's'} where only cards seen in the game log count as sided in.`
}

/** What a log-only game limits: Seen, or the sideboard numbers. */
export type EstimatedKind = 'seen' | 'sideboard'

/** The estimated kind of a column, or null when imported games do not limit it. */
export function estimatedKind(key: string): EstimatedKind | null {
  if (key === 'seenPct' || key === 'seenWinPct') return 'seen'
  if (key === 'sbOutPct' || key === 'sbInPct') return 'sideboard'
  return null
}

/** The muted note above the table when games come from MTGO's game log alone. */
export function logOnlyNote(logOnly: number): string {
  const subject = logOnly === 1 ? '1 game here is from an imported match' : `${logOnly} games here are from imported matches`
  return `${subject} with only MTGO's game log. Seen counts only cards that were cast or played, and sideboard numbers are partial.`
}

/** The tooltip behind the "est." marker on a column. */
export function estimatedTooltip(kind: EstimatedKind, logOnly: number): string {
  const one = logOnly === 1
  const origin = `${logOnly} game${one ? '' : 's'} here ${one ? 'is' : 'are'} from ${one ? 'an imported match' : 'imported matches'} with only MTGO's game log`
  if (kind === 'seen') {
    return `Estimated. ${origin}, which never shows your hand, so a card counts as seen only when it was cast or played.`
  }
  return `Estimated. ${origin}, so sideboarding is only visible for cards that show up in the log.`
}
