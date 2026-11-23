import type { PlatformView } from '@/bindings'

/** Each platform with a match in progress, once (`livePlatforms`: token to platform). */
export function platformsWithLiveMatch(live: Map<string, PlatformView>): PlatformView[] {
  return [...new Set(live.values())]
}

/** MTGO is the one platform with leagues, Limited and its own log words; Arena and Endstep share the other side of each branch. */
export function isMtgo(platform: PlatformView): boolean {
  return platform === 'mtgo'
}

/** Endstep has no manual match (the store refuses one). */
export function canAddManualMatch(platform: PlatformView): boolean {
  return platform !== 'endstep'
}
