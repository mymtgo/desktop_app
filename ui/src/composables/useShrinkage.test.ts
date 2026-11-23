import { describe, expect, it } from 'vitest'
import { ref } from 'vue'
import { useShrinkage } from './useShrinkage'

const stat = {
  keptWon: 0,
  keptLost: 0,
  castWon: 2,
  castLost: 0,
  seenWon: 1,
  seenLost: 3,
  pregameWon: 0,
  pregameLost: 0,
}

describe('useShrinkage', () => {
  it('shrinks toward the prior and reports the samples and raw rates', () => {
    const strength = ref(50)
    const rows = useShrinkage({ stats: [stat], prior: 0.48, strength, perspective: 'mine' })
    const [row] = rows.value
    expect(row.samples).toEqual({ kept: 0, cast: 2, seen: 4, pregame: 0 })
    expect(row.rawRates).toEqual({ kept: null, cast: 1, seen: 0.25, pregame: null })
    expect(row.shrunk.cast).toBeCloseTo(0.5, 10)
    expect(row.shrunk.kept).toBe(0.48)

    strength.value = 0
    expect(rows.value[0].shrunk.cast).toBe(1)
  })

  it('shows their cards from their side', () => {
    const rows = useShrinkage({ stats: [stat], prior: 0.5, strength: 0, perspective: 'theirs' })
    expect(rows.value[0].shrunk.cast).toBe(0)
    expect(rows.value[0].rawRates.seen).toBe(0.75)
  })
})
