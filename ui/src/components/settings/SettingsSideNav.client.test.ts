import { describe, expect, it, vi } from 'vitest'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const route = vi.hoisted(() => ({ path: '/settings/general' }))
const context = vi.hoisted(() => ({ platform: 'mtgo' as 'mtgo' | 'arena' }))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub, useRoute: () => route }
})

vi.mock('@/composables/usePlatformContext', async () => {
  const { hasFeature } = await import('@/lib/platformFeatures')
  return {
    usePlatformContext: () => ({
      hasFeature: (feature: import('@/lib/platformFeatures').Feature) => hasFeature(context.platform, feature),
    }),
  }
})

const { default: SettingsSideNav } = await import('./SettingsSideNav.vue')

function links(path: string) {
  route.path = path
  const mounted = mountFake(SettingsSideNav)
  expect(mounted.warnings).toEqual([])
  return { mounted, links: findAll(mounted.root, (node) => node.tag === 'a') }
}

describe('the settings side nav', () => {
  it('hides Overlays in the Arena context', () => {
    context.platform = 'arena'
    const { links: found } = links('/settings/general')
    context.platform = 'mtgo'
    expect(found.map((link) => textOf(link).trim())).not.toContain('Overlays')
    expect(found.map((link) => textOf(link).trim())).toContain('Storage')
  })

  it('shows the header and the pages in 0.x order, Archives after Storage', () => {
    const { mounted, links: found } = links('/settings/general')
    const text = textOf(mounted.root)
    expect(text).toContain('Settings')
    expect(text).toContain('Configure how MyMTGO tracks and displays your games.')
    expect(found.map((link) => textOf(link).trim())).toEqual([
      'General',
      'Account',
      'Overlays',
      'Storage',
      'Archives',
      'Replays',
      'Integrations',
      'Data & Privacy',
      'Advanced',
    ])
    expect(found.map((link) => link.props.href)).toEqual([
      '/settings/general',
      '/settings/account',
      '/settings/overlays',
      '/settings/storage',
      '/settings/archives',
      '/settings/replays',
      '/settings/integrations',
      '/settings/privacy',
      '/settings/advanced',
    ])
  })

  it('marks only the current page active', () => {
    const { links: found } = links('/settings/storage')
    const active = found.filter((link) => hasClass(link, 'nav-item-active')).map((link) => textOf(link).trim())
    expect(active).toEqual(['Storage'])
    expect(found.filter((link) => hasClass(link, 'nav-item-inactive'))).toHaveLength(8)
  })
})
