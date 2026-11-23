import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { UpdateStatus } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ update: null as UpdateStatus | null }))

vi.mock('@/lib/queries', () => ({ useUpdateStatus: () => ({ data: ref(state.update) }) }))
vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
// The dialog renders its content in place, open or not, so the licence text is visible to the test.
vi.mock('@/components/ui/dialog', async () => {
  const { defineComponent, h } = await import('vue')
  const pass = (tag: string) =>
    defineComponent({
      props: { open: { type: Boolean, default: undefined } },
      setup: (props, { slots }) => () => h(tag, { open: props.open }, slots.default?.()),
    })
  return {
    Dialog: pass('dialog'),
    DialogContent: pass('dialog-content'),
    DialogHeader: pass('dialog-header'),
    DialogTitle: pass('dialog-title'),
    DialogDescription: pass('dialog-description'),
  }
})

const { default: AboutCard } = await import('./AboutCard.vue')

function mountCard(version: string) {
  const mounted = mountFake(AboutCard, { version })
  expect(mounted.warnings).toEqual([])
  return mounted
}

function status(channel: UpdateStatus['channel'], current: string): UpdateStatus {
  return { current, available: null, phase: 'up_to_date', checkedAtMs: null, error: null, active: true, channel }
}

describe('the About card', () => {
  beforeEach(() => {
    state.update = null
  })

  // Release spec section 9.
  it('shows the version with its channel and the Wizards of the Coast notice', () => {
    const text = textOf(mountCard('1.0.0-beta.2').root)
    expect(text).toContain('About')
    expect(text).toContain('MyMTGO v1.0.0-beta.2 (edge)')
    expect(text).toContain('MyMTGO is not affiliated with, endorsed by, or sponsored by Wizards of the Coast.')
    expect(textOf(mountCard('1.0.0').root)).toContain('MyMTGO v1.0.0 (stable)')
  })

  // The update channel (W4) is the user's choice, not only the build's.
  it("shows the update status's channel once it is known", () => {
    state.update = status('edge', '1.0.0')
    expect(textOf(mountCard('1.0.0').root)).toContain('MyMTGO v1.0.0 (edge)')
    state.update = status('stable', '1.0.0-beta.2')
    expect(textOf(mountCard('1.0.0-beta.2').root)).toContain('MyMTGO v1.0.0-beta.2 (stable)')
  })

  it('links to the third-party licences page', () => {
    const links = findAll(mountCard('1.0.0').root, (node) => node.tag === 'a')
    const link = links.find((node) => textOf(node).trim() === 'Third-party licences')
    expect(link?.props.href).toBe('/settings/advanced/licences')
  })

  it('opens the MyMTGO licence text from the Licence button', async () => {
    const mounted = mountCard('1.0.0')
    const dialog = () => findAll(mounted.root, (node) => node.tag === 'dialog')[0]
    expect(dialog().props.open).toBe(false)
    expect(textOf(dialog())).toContain('MyMTGO Source-Available Licence, Version 1.0')
    const button = findAll(mounted.root, (node) => node.tag === 'button').find(
      (node) => textOf(node).trim() === 'Licence',
    )
    expect(button).toBeDefined()
    ;(button!.props.onClick as () => void)()
    await mounted.settle()
    expect(dialog().props.open).toBe(true)
  })
})
