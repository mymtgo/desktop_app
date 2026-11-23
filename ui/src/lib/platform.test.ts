import { describe, expect, it } from 'vitest'
import type { PlatformView } from '@/bindings'
import { canAddManualMatch, platformsWithLiveMatch } from './platform'

describe('platformsWithLiveMatch', () => {
  it('lists each platform with an in-progress match once', () => {
    const live = new Map<string, PlatformView>()
    expect(platformsWithLiveMatch(live)).toEqual([])
    live.set('a', 'endstep')
    live.set('b', 'endstep')
    expect(platformsWithLiveMatch(live)).toEqual(['endstep'])
    live.set('c', 'mtgo')
    expect(platformsWithLiveMatch(live)).toEqual(['endstep', 'mtgo'])
  })
})

describe('canAddManualMatch', () => {
  it('is false on Endstep only', () => {
    expect(canAddManualMatch('mtgo')).toBe(true)
    expect(canAddManualMatch('arena')).toBe(true)
    expect(canAddManualMatch('endstep')).toBe(false)
  })
})
