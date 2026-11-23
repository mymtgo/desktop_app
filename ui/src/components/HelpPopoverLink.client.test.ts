import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ pushed: [] as unknown[] }))

vi.mock('vue-router', () => ({
  useRouter: () => ({
    push: async (to: unknown) => {
      state.pushed.push(to)
    },
  }),
}))
vi.mock('@/bindings', () => ({ commands: {} }))
vi.mock('@/composables/useReportBundle', () => ({
  useReportBundle: () => ({
    busy: ref(false),
    error: ref({ message: 'x', showSettingsLink: true }),
    download: async () => {},
  }),
}))
// The popover as plain elements: the fake renderer has no portal or popper.
vi.mock('@/components/ui/popover', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) => defineComponent({ setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return { Popover: plain('popover'), PopoverTrigger: plain('popover-trigger'), PopoverContent: plain('popover-content') }
})

const { default: HelpPopover } = await import('./HelpPopover.vue')

describe('the Get help popover', () => {
  // Release spec section 8.1: "Open settings" goes to the Storage page (W1), where the log path is set.
  it('opens the Storage settings page from Open settings', () => {
    const mounted = mountFake(HelpPopover, {})
    const button = findAll(mounted.root, (node) => node.tag === 'button').find(
      (node) => textOf(node).trim() === 'Open settings',
    )
    expect(button).toBeDefined()
    ;(button!.props.onClick as () => void)()
    expect(state.pushed).toEqual([{ name: 'settings-storage' }])
  })
})
