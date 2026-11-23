import { describe, expect, it } from 'vitest'
import { mountFake, textOf } from '@/replay/replayTestHarness'
import { entryFor } from './index'

const { default: Page } = await import('./1.3.vue')

describe('What\'s new in 1.3', () => {
  it('is the entry for 1.3 releases and their candidates', () => {
    expect(entryFor('1.3.0-rc.1')?.version).toBe('1.3')
    expect(entryFor('1.3.0')?.version).toBe('1.3')
    expect(entryFor('1.2.1')?.version).toBe('1.2')
  })

  it('groups the changes, a sentence each', () => {
    const mounted = mountFake(Page)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    for (const feature of ['MyMTGO 1.3', 'New', 'MTG Arena', 'Endstep', 'Everywhere', 'Changes', 'Reporting problems', 'MyMTGO for Mac (Apple Silicon)', 'Sikarugir, Kegworks', 'manual folder settings', 'on every Space', 'Vivaldi', 'macOS may ask you to allow it', 'sync across your devices', 'even one started on your phone', 'Limited decks sync automatically for Supporters', 'Open data console', 'export it as CSV or JSON', 'which hidden card they put on the bottom', 'no longer shows a library position', 'follow copies of spells', 'Archangel of Tithes', 'ends without a result', 'status of an Arena limited run', 'renamed before it signed in', 'One damaged entry', 'claim on a manual match', 'keeps games 2 and 3', 'keeps the store links', 'matches appear after your first one', 'extension is not connected', 'starting hand size and discards', 'activated ability', 'Ranked or Casual', 'poison counters', 'back of double-faced cards', 'same winner as the result', 'backlog of captures', 'only for browsers you have installed', 'leaves a page the new platform does not have', 'match in progress from the moment', 'hidden match you show again', 'No matches with this deck yet', 'no longer stored or simply has no replay', 'only a sliver left on screen', 'cannot be used yet', 'up to date instead of showing an update error', 'extension 1.0.2']) {
      expect(text).toContain(feature)
    }
    expect(text).not.toContain('\u2014')
  })
})
