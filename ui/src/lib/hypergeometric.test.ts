import { describe, expect, it } from 'vitest'
import { formatPercent, hypergeometric } from './hypergeometric'

// Expected values come from running 0.x `composables/useHypergeometric.ts`
// on the same inputs.

describe('hypergeometric', () => {
  it('gives the odds of drawing one of four copies in an opening seven', () => {
    const result = hypergeometric(60, 4, 7, 1)
    expect(result.atLeast).toBeCloseTo(0.39949962574466236, 12)
    expect(result.exactly).toBeCloseTo(0.33628020958298466, 12)
    expect(result.atMost).toBeCloseTo(0.9367805838383155, 12)
    expect(result.zero).toBeCloseTo(0.6005003742553309, 12)
  })

  it('matches the exact binomial maths for a single copy', () => {
    // 1 - C(59, 7) / C(60, 7) = 7 / 60
    expect(hypergeometric(60, 1, 7, 1).atLeast).toBeCloseTo(7 / 60, 12)
    expect(hypergeometric(60, 1, 7, 1).atMost).toBe(1)
  })

  it('handles larger wanted counts', () => {
    const two = hypergeometric(60, 4, 7, 2)
    expect(two.atLeast).toBeCloseTo(0.0632194161616777, 12)
    expect(two.exactly).toBeCloseTo(0.059343566396997206, 12)
    expect(two.atMost).toBeCloseTo(0.9961241502353128, 12)

    const lands = hypergeometric(40, 17, 7, 3)
    expect(lands.atLeast).toBeCloseTo(0.6493416493416464, 12)
    expect(lands.exactly).toBeCloseTo(0.32297479665900586, 12)
    expect(lands.atMost).toBeCloseTo(0.6736331473173517, 12)
    expect(lands.zero).toBeCloseTo(0.01314968814968796, 12)
  })

  it('is all zeros without a population or a sample', () => {
    const empty = { atLeast: 0, exactly: 0, atMost: 0, zero: 0 }
    expect(hypergeometric(0, 4, 7, 1)).toEqual(empty)
    expect(hypergeometric(60, 4, 0, 1)).toEqual(empty)
    expect(hypergeometric(Number.NaN, 4, 7, 1)).toEqual(empty)
    expect(hypergeometric(-5, 4, 7, 1)).toEqual(empty)
    expect(hypergeometric(Number.POSITIVE_INFINITY, 4, 7, 1)).toEqual(empty)
  })

  it('clamps the wanted count to what can be drawn, as 0.x does', () => {
    // No successes in the population: the wanted count drops to 0.
    expect(hypergeometric(60, 0, 7, 1)).toEqual({ atLeast: 1, exactly: 1, atMost: 1, zero: 1 })
    // Wanting more than the copies: the wanted count drops to 4.
    const many = hypergeometric(60, 4, 7, 9)
    expect(many.atLeast).toBeCloseTo(0.00007177499564223108, 15)
    expect(many.exactly).toBeCloseTo(0.00007177499564223108, 15)
    // Everything larger than the population is cut down to it.
    expect(hypergeometric(10, 20, 30, 40)).toEqual({ atLeast: 1, exactly: 1, atMost: 1, zero: 0 })
    expect(hypergeometric(7, 4, 7, 4)).toEqual({ atLeast: 1, exactly: 1, atMost: 1, zero: 0 })
  })

  it('rounds inputs down and treats a negative wanted count as 0', () => {
    expect(hypergeometric(60.9, 4.7, 7.2, 1.9)).toEqual(hypergeometric(60, 4, 7, 1))
    const none = hypergeometric(60, 4, 7, -1)
    expect(none.atLeast).toBeCloseTo(1, 12)
    expect(none.exactly).toBeCloseTo(0.6005003742553309, 12)
  })

  it('stays finite for large populations', () => {
    const big = hypergeometric(100000, 50000, 5000, 2500)
    expect(big.atLeast).toBeCloseTo(0.5057881727946173, 9)
    expect(big.exactly).toBeCloseTo(0.011576345629601487, 9)
    expect(big.atMost).toBeCloseTo(0.5057881727946179, 9)
    expect(big.zero).toBe(0)
  })

  it('never goes above 1', () => {
    const result = hypergeometric(60, 4, 7, 0)
    expect(result.atLeast).toBeLessThanOrEqual(1)
    expect(result.atMost).toBeLessThanOrEqual(1)
  })
})

describe('formatPercent', () => {
  it('shows a probability with one decimal, as the 0.x calculator', () => {
    expect(formatPercent(0.39949962574466236)).toBe('39.9%')
    expect(formatPercent(1)).toBe('100.0%')
    expect(formatPercent(0)).toBe('0.0%')
    expect(formatPercent(0.1234)).toBe('12.3%')
  })
})
