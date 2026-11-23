import { describe, expect, it } from 'vitest'
import { perspectiveAdjust, rawRate, shrunkRate } from './shrinkage'

describe('shrunkRate', () => {
  it('is the prior when there are no games and no strength', () => {
    expect(shrunkRate({ wins: 0, games: 0, prior: 0.48, strength: 0 })).toBe(0.48)
  })

  it('is the prior with no games whatever the strength', () => {
    expect(shrunkRate({ wins: 0, games: 0, prior: 0.48, strength: 50 })).toBe(0.48)
  })

  it('pulls a small sample toward the prior', () => {
    // (2 + 50 * 0.48) / (2 + 50) = 0.5
    expect(shrunkRate({ wins: 2, games: 2, prior: 0.48, strength: 50 })).toBeCloseTo(0.5, 10)
  })

  it('gives the raw rate at strength 0', () => {
    expect(shrunkRate({ wins: 3, games: 4, prior: 0.2, strength: 0 })).toBe(0.75)
  })

  it('clamps to 0 and 1', () => {
    expect(shrunkRate({ wins: 5, games: 2, prior: 0.5, strength: 0 })).toBe(1)
    expect(shrunkRate({ wins: -3, games: 2, prior: 0.5, strength: 0 })).toBe(0)
    expect(shrunkRate({ wins: 0, games: 0, prior: 7, strength: 5 })).toBe(1)
  })

  it('treats a non-finite prior as one half', () => {
    expect(shrunkRate({ wins: 0, games: 0, prior: Number.NaN, strength: 5 })).toBe(0.5)
  })
})

describe('rawRate and perspectiveAdjust', () => {
  it('has no raw rate without games', () => {
    expect(rawRate(0, 0)).toBeNull()
    expect(rawRate(1, 4)).toBe(0.25)
  })

  it('inverts a rate for their cards', () => {
    expect(perspectiveAdjust(0.25, 'theirs')).toBe(0.75)
    expect(perspectiveAdjust(0.25, 'mine')).toBe(0.25)
  })
})
