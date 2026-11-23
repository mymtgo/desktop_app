import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { findAll, mountFake } from '@/replay/replayTestHarness'

vi.mock('@/components/UpdatesCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('updates-card') }) }
})
vi.mock('@/components/settings/AccountsCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('accounts-card') }) }
})
vi.mock('@/components/settings/BackgroundCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('background-card') }) }
})
vi.mock('@/components/settings/InterfaceScaleCard.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ setup: () => () => h('interface-scale-card') }) }
})

vi.mock('@/composables/useWhatsNew', () => ({ useWhatsNewUrl: () => ref('/whats-new') }))

const { default: GeneralPage } = await import('./GeneralPage.vue')

describe('the General settings page', () => {
  // 0.x Pages/GeneralControllerTest.
  it('shows the Updates card, then Accounts, then Appearance, then Background', () => {
    const mounted = mountFake(GeneralPage)
    expect(mounted.warnings).toEqual([])
    const tags = ['updates-card', 'accounts-card', 'interface-scale-card', 'background-card']
    const cards = findAll(mounted.root, (node) => tags.includes(node.tag))
    expect(cards.map((node) => node.tag)).toEqual(tags)
  })

  // Release spec section 7: the Updates card links What's new when the page exists.
  it('passes the What\'s new link to the Updates card', () => {
    const mounted = mountFake(GeneralPage)
    const [card] = findAll(mounted.root, (node) => node.tag === 'updates-card')
    expect(card!.props['whats-new-url']).toBe('/whats-new')
  })
})
