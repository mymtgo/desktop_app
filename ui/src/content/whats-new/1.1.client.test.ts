import { describe, expect, it } from 'vitest'
import { mountFake, textOf } from '@/replay/replayTestHarness'
import { entryFor } from './index'

const { default: Page } = await import('./1.1.vue')

describe('What\'s new in 1.1', () => {
  it('is the entry for 1.1 releases', () => {
    expect(entryFor('1.1.0')?.version).toBe('1.1')
    expect(entryFor('1.1.1')?.version).toBe('1.1')
    expect(entryFor('1.1.2')?.version).toBe('1.1')
    expect(entryFor('1.0.4')?.version).toBe('1.0')
  })

  it('groups the changes, a sentence each', () => {
    const mounted = mountFake(Page)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    for (const feature of ['MyMTGO 1.1', 'New', 'Improvements', 'Changes', 'Fixes', 'Archetypes offline', 'Detect archetypes', 'Custom badge', 'Right-click a deck', 'match page', 'maindeck is complete', 'tournament round has started', 'Card Stats tables scroll', 'leaves the league overlay']) {
      expect(text).toContain(feature)
    }
    expect(text).not.toContain('—')
  })
})
