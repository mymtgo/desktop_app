import { normalizeType, TYPE_ORDER } from '@/lib/cardTypes'

/**
 * Overlay card lists under type headings (0.x `useCardTypeGroups`): full
 * type lines ("Legendary Creature, Eldrazi" with its dash) collapse onto the canonical
 * types, shown in Creature, Planeswalker, Battle, Instant, Sorcery,
 * Enchantment, Artifact, Land order, then any other type; a card without
 * one is "Other". Within a type the caller's order stands.
 */
export function typeOf(raw: string | null | undefined): string {
  if (!raw) return 'Other'
  return normalizeType(raw)
}

export function groupByType<T>(cards: T[], type: (card: T) => string | null | undefined): Record<string, T[]> {
  const merged: Record<string, T[]> = {}
  for (const card of cards) {
    const key = typeOf(type(card))
    ;(merged[key] ??= []).push(card)
  }
  return Object.fromEntries(Object.entries(merged).sort(([a], [b]) => (TYPE_ORDER[a] ?? 99) - (TYPE_ORDER[b] ?? 99)))
}
