import { describe, expect, it } from 'vitest'
import { rankIconUrl, rankSteps } from './rankIcons'

describe('rankIconUrl', () => {
  it('bronze 4 step 0 is the bronze tier 4 icon', () => {
    expect(rankIconUrl('constructed', 0)).toContain('constructed_bronze_t4')
  })

  it('diamond 1 step 5 is the diamond tier 1 icon', () => {
    // Diamond starts at 96; step 5 of the class is still tier 4, so use the top tier.
    expect(rankIconUrl('constructed', 96 + 18)).toContain('constructed_diamond_t1')
    expect(rankIconUrl('constructed', 96 + 23)).toContain('constructed_diamond_t1')
  })

  it('120 is mythic', () => {
    expect(rankIconUrl('constructed', 120)).toContain('constructed_mythic')
    expect(rankIconUrl('constructed', 400)).toContain('constructed_mythic')
  })

  it('limited lines use the limited icons', () => {
    expect(rankIconUrl('limited', 30)).toContain('limited_silver_t3')
  })

  it('spark, below bronze, has an icon per five-step tier', () => {
    expect(rankIconUrl('constructed', -20)).toContain('constructed_spark_t4')
    expect(rankIconUrl('constructed', -16)).toContain('constructed_spark_t4')
    expect(rankIconUrl('constructed', -15)).toContain('constructed_spark_t3')
    expect(rankIconUrl('constructed', -6)).toContain('constructed_spark_t2')
    expect(rankIconUrl('constructed', -5)).toContain('constructed_spark_t1')
    expect(rankIconUrl('constructed', -1)).toContain('constructed_spark_t1')
    expect(rankIconUrl('constructed', 0)).toContain('constructed_bronze_t4')
  })

  it('every class and tier has a file', () => {
    for (let steps = -20; steps < 0; steps += 1) {
      expect(rankIconUrl('constructed', steps), `constructed ${steps}`).toMatch(/\.webp|data:/)
    }
    for (const ladder of ['constructed', 'limited'] as const) {
      for (let steps = 0; steps <= 120; steps += 1) {
        expect(rankIconUrl(ladder, steps), `${ladder} ${steps}`).toMatch(/\.webp|data:/)
      }
    }
  })
})

describe('rankSteps', () => {
  it.each([
    ['Spark', 4, 0, -20],
    ['Spark', 3, 0, -15],
    ['Spark', 1, 4, -1],
    ['Bronze', 4, 0, 0],
    ['Bronze', 1, 5, 18 + 5],
    ['Gold', 2, 3, 60 + 3],
    ['Diamond', 1, 0, 114],
    ['Mythic', 1, 0, 120],
  ])('%s %i step %i is %i steps', (rankClass, level, step, steps) => {
    expect(rankSteps(rankClass, level, step)).toBe(steps)
  })

  it('is null for an unknown class', () => {
    expect(rankSteps('Wood', 1, 0)).toBeNull()
  })

  it('clamps a step past the tier so it cannot show the next tier', () => {
    expect(rankSteps('Gold', 2, 9)).toBe(60 + 5)
    expect(rankSteps('Gold', 2, -1)).toBe(60)
    // Spark tiers have five steps.
    expect(rankSteps('Spark', 4, 7)).toBe(-20 + 4)
  })
})
