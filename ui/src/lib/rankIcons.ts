/**
 * Arena's own rank icons (copied from the client's ranked atlas, owner
 * decision), one per ladder, class and tier, picked from a step value.
 * Spark (constructed only, below Bronze, tiers of five steps) is
 * `constructed_spark_t<tier>`, from the client's `PlayerRankSprites` bundle.
 */
import type { LadderView } from '@/bindings'

const STEPS_PER_TIER = 6
/** Steps per class (four tiers of six steps). */
export const STEPS_PER_CLASS = 24
export const MYTHIC_STEPS = 120
/** Spark's tiers have five steps; Spark 4 step 0 is `-SPARK_STEPS`. */
const SPARK_STEPS_PER_TIER = 5
export const SPARK_STEPS = 4 * SPARK_STEPS_PER_TIER
const CLASS_FILES = ['bronze', 'silver', 'gold', 'platinum', 'diamond']

const icons = import.meta.glob('@/assets/ranks/*.webp', { eager: true, import: 'default' }) as Record<string, string>

/** The icon file's name (without extension) for a step value. */
export function rankIconName(ladder: LadderView, steps: number): string {
  if (steps >= MYTHIC_STEPS) return `${ladder}_mythic`
  if (steps < 0) {
    const fromSpark = Math.max(0, steps + SPARK_STEPS)
    return `${ladder}_spark_t${4 - Math.floor(fromSpark / SPARK_STEPS_PER_TIER)}`
  }
  const clamped = Math.max(0, steps)
  const rankClass = CLASS_FILES[Math.floor(clamped / STEPS_PER_CLASS)]
  const tier = 4 - Math.floor((clamped % STEPS_PER_CLASS) / STEPS_PER_TIER)
  return `${ladder}_${rankClass}_t${tier}`
}

const iconsByName = new Map<string, string>()
for (const [path, url] of Object.entries(icons)) {
  const file = path.split('/').pop() ?? ''
  iconsByName.set(file.replace(/\.webp$/, ''), url)
}

/** The icon's URL for a file name (without extension), if there is one. */
export function rankIconUrlByName(name: string): string | undefined {
  return iconsByName.get(name)
}

/** The icon's URL for a step value. */
export function rankIconUrl(ladder: LadderView, steps: number): string {
  return rankIconUrlByName(rankIconName(ladder, steps)) ?? ''
}

/**
 * Steps from Bronze 4 step 0 for a class, level (4 lowest) and step; Spark is
 * negative (`arena_match::rank_line::rank_steps`), Mythic is `MYTHIC_STEPS`.
 * Null for an unknown class.
 */
export function rankSteps(rankClass: string, level: number, step: number): number | null {
  if (rankClass === 'Mythic') return MYTHIC_STEPS
  const tierInClass = 4 - Math.min(4, Math.max(1, level))
  if (rankClass === 'Spark') {
    return -SPARK_STEPS + tierInClass * SPARK_STEPS_PER_TIER + Math.min(SPARK_STEPS_PER_TIER - 1, Math.max(0, step))
  }
  const classIndex = CLASS_FILES.indexOf(rankClass.toLowerCase())
  if (classIndex === -1) return null
  const tier = classIndex * 4 + tierInClass
  // A step past the tier would read as the next tier's icon.
  return tier * STEPS_PER_TIER + Math.min(STEPS_PER_TIER - 1, Math.max(0, step))
}
