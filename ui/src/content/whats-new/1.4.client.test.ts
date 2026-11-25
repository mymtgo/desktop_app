import { describe, expect, it } from 'vitest'
import { mountFake, textOf } from '@/replay/replayTestHarness'
import { entryFor } from './index'

const { default: Page } = await import('./1.4.vue')

describe('What\'s new in 1.4', () => {
  it('is the entry for 1.3 releases and their candidates', () => {
    expect(entryFor('1.4.0-rc.1')?.version).toBe('1.4')
    expect(entryFor('1.4.0')?.version).toBe('1.4')
    expect(entryFor('1.3.1')?.version).toBe('1.3')
  })

  it('groups the changes, a sentence each', () => {
    const mounted = mountFake(Page)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    for (const feature of ['MyMTGO 1.4', 'New', 'MTG Arena', 'Endstep', 'Everywhere', 'Changes', 'Reporting problems', 'Showcase Challenges', 'strip under the card', 'live preview', 'instead of always saying CHALLENGER', 'dots sized by matches', 'last 7 days played', 'archetypes you played', '80% to 150%', 'Spark rank', 'second account behind', 'end on the clock', 'Restore brings it back', 'grey and dashed', 'says what it lists', 'github.com/mymtgo/desktop_app', 'exactly the size of the card', 'cumulative win rate line', 'Challenge or Qualifier', 'MyMTGO Wrapped arrives on 1 December', 'Share anonymous usage data']) {
      expect(text).toContain(feature)
    }
    expect(text).not.toContain('\u2014')
  })
})
