import { describe, expect, it, vi } from 'vitest'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const route = vi.hoisted(() => ({ path: '/settings/general', meta: {} as Record<string, unknown> }))

vi.mock('vue-router', async () => {
  const { RouterLinkStub, RouterViewStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub, RouterView: RouterViewStub, useRoute: () => route }
})

vi.mock('@/composables/usePlatformContext', () => ({ usePlatformContext: () => ({ hasFeature: () => true }) }))

const { default: SettingsLayout } = await import('./SettingsLayout.vue')

function header(path: string) {
  route.path = path
  const mounted = mountFake(SettingsLayout)
  expect(mounted.warnings).toEqual([])
  const h1 = findAll(mounted.root, (node) => node.tag === 'h1')
  const views = findAll(mounted.root, (node) => node.tag === 'router-view')
  expect(views).toHaveLength(1)
  return { title: h1.map((node) => textOf(node).trim()), text: textOf(mounted.root) }
}

describe('the settings layout', () => {
  it.each([
    ['/settings/general', 'General', 'Accounts and how MyMTGO runs on your machine.'],
    ['/settings/account', 'Account', 'Back your matches up to MyMTGO and keep them in sync across your devices.'],
    ['/settings/overlays', 'Overlays', 'Always-on-top windows shown during leagues, matches and drafts.'],
    ['/settings/storage', 'Storage', 'Where MTGO files live and what MyMTGO keeps on disk.'],
    ['/settings/privacy', 'Data & Privacy', 'What leaves this device and what comes back in.'],
    ['/settings/advanced', 'Advanced', 'Developer tools and diagnostics.'],
  ])('heads %s with its title and description', (path, title, description) => {
    const shown = header(path)
    expect(shown.title).toEqual([title])
    expect(shown.text).toContain(description)
  })

  it('keeps a settings page in a max-w-3xl column unless its route is wide', () => {
    const wrapper = (wide: boolean) => {
      route.path = '/settings/archives'
      route.meta = wide ? { wide: true } : {}
      const mounted = mountFake(SettingsLayout)
      const view = findAll(mounted.root, (node) => node.tag === 'router-view')[0]
      return view.parent as NonNullable<typeof view.parent>
    }
    expect(hasClass(wrapper(false), 'max-w-3xl')).toBe(true)
    expect(hasClass(wrapper(true), 'max-w-3xl')).toBe(false)
    route.meta = {}
  })
})
