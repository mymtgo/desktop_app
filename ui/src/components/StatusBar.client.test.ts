import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { findAll, mountFake } from '@/replay/replayTestHarness'

vi.mock('@/lib/queries', () => ({
  useStatus: () => ({ data: ref(undefined) }),
  useIntegrationsQuery: () => ({ data: ref(undefined) }),
}))
vi.mock('@/composables/usePlatformContext', () => ({ usePlatformContext: () => ({ platform: ref('arena') }) }))
vi.mock('@/composables/useOfflineMode', () => ({ useOfflineMode: () => ref(false) }))
vi.mock('@/composables/useWhatsNew', () => ({ useWhatsNewUrl: () => ref('/whats-new') }))
vi.mock('@/components/StatusBarVersion.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('status-bar-version') }) }
})
vi.mock('@/components/HelpPopover.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('help-popover') }) }
})

const { default: StatusBar } = await import('./StatusBar.vue')

describe('the status bar', () => {
  // W4's version (update ready, downloading or the version) sits where the version was, then W6's Get help.
  it('ends with the version then Get help', () => {
    const mounted = mountFake(StatusBar)
    expect(mounted.warnings).toEqual([])
    const [footer] = findAll(mounted.root, (node) => node.tag === 'footer')
    const tags = footer!.children.map((node) => node.tag).filter((tag) => tag !== 'div' && tag !== 'span')
    expect(tags.slice(-2)).toEqual(['status-bar-version', 'help-popover'])
  })

  // Release spec section 7: the version links What's new when the page exists.
  it('passes the What\'s new link to the version', () => {
    const mounted = mountFake(StatusBar)
    const [version] = findAll(mounted.root, (node) => node.tag === 'status-bar-version')
    expect(version!.props['whats-new-url']).toBe('/whats-new')
  })
})
