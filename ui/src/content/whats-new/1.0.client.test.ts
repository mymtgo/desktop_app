import { describe, expect, it, vi } from 'vitest'
import { mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: Page } = await import('./1.0.vue')

describe('What\'s new in 1.0', () => {
  // The owner dropped the SmartScreen guidance from this page (2026-10-19):
  // players moving from 0.x already have MyMTGO installed.
  it('has the import, Linux and the update channels, and no SmartScreen note', () => {
    const mounted = mountFake(Page)
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    expect(text).not.toContain('Windows may warn')
    expect(text).toContain('Import')
    expect(text).toContain('Linux')
    expect(text).toContain('Enable edge updates')
    expect(text).not.toContain('\u2014')
  })

  it('lists what is new since 0.x', () => {
    const text = textOf(mountFake(Page).root).replace(/\s+/g, ' ')
    expect(text).toContain('New since MyMTGO 0.x')
    for (const feature of ['MyMTGO 1.0', 'Dashboard', 'Manage your matches and leagues', 'Versus overlay', 'Metagame and archetypes']) {
      expect(text).toContain(feature)
    }
    expect(text).not.toContain("MTGO's own game history")
  })

  it('has the smaller improvements under the features', () => {
    const text = textOf(mountFake(Page).root).replace(/\s+/g, ' ')
    for (const point of ['Faster and lighter', 'A sturdier foundation', 'Updates arrive quicker', 'Linux']) {
      expect(text).toContain(point)
    }
    expect(text).not.toContain('Rust')
  })
})
