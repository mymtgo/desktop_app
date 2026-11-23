import { describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'
import { entryFor } from './index'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: Page } = await import('./1.2.vue')

describe('What\'s new in 1.2', () => {
  it('is the entry for 1.2 releases and their candidates', () => {
    expect(entryFor('1.2.0-rc.1')?.version).toBe('1.2')
    expect(entryFor('1.2.0')?.version).toBe('1.2')
    expect(entryFor('1.2.1')?.version).toBe('1.2')
    expect(entryFor('1.1.2')?.version).toBe('1.1')
  })

  it('groups the changes, a sentence each', () => {
    const mounted = mountFake(Page)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    for (const feature of ['MyMTGO 1.2', 'MyMTGO has evolved', 'Set up Arena and Endstep', 'New', 'Improvements', 'Changes', 'Reporting problems', 'MTG Arena', 'Bo1 and Bo3 matches are tracked', 'Bo3 | Bo1 switch', 'a tag for each kind played', 'Switch between MTGO and Arena', 'Rank over the season', 'Replays of your Arena games', 'Overlays stay MTGO only', 'Trends tab', 'follow the timeframe you pick', 'sent to MyMTGO for the metagame', 'deck\'s cover', 'Commander section first', 'includes Arena\'s log', 'Endstep', 'Integrations page', 'Arena starts switched off', 'can be cropped to fit', 'Competitive Sealed', 'entering a league again starts a new run', 'archetype guessed from their cards', 'Edit match', 'never rounded up', 'sent back with a mulligan', 'arrives with the archetype', 'where a match is in progress']) {
      expect(text).toContain(feature)
    }
    expect(text).not.toContain('—')
  })

  it('links the hero button to Settings, Integrations', () => {
    const mounted = mountFake(Page)
    const links = findAll(mounted.root, (n) => n.tag === 'a').map((n) => n.props.href)
    expect(links).toContain('/settings/integrations')
  })
})
