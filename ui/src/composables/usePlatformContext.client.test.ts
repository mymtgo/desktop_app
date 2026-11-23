import { afterEach, describe, expect, it, vi } from 'vitest'
import type { PlatformContextView } from '@/bindings'
import { mountFake } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  context: undefined as unknown as { value: PlatformContextView | undefined },
  route: { value: { name: 'leagues' as string } },
  replaced: [] as unknown[],
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({
    currentRoute: state.route,
    replace: (to: unknown) => {
      state.replaced.push(to)
      return Promise.resolve()
    },
  }),
}))
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  state.context = ref<PlatformContextView | undefined>(undefined)
  return { usePlatformContextQuery: () => ({ data: state.context }) }
})

const { defineComponent, h } = await import('vue')
const { useLeaveHiddenPages } = await import('./usePlatformContext')

const Host = defineComponent({
  setup() {
    useLeaveHiddenPages()
    return () => h('host')
  },
})

const context = (current: PlatformContextView['current']): PlatformContextView => ({
  current,
  mtgo: true,
  arena: true,
  endstep: false,
  switchable: true,
})

const open: { unmount: () => void }[] = []
afterEach(() => {
  open.splice(0).forEach((mounted) => mounted.unmount())
  state.replaced = []
  state.context.value = undefined
})

describe('useLeaveHiddenPages', () => {
  it('leaves a page the new platform hides for the dashboard', async () => {
    state.route.value = { name: 'leagues' }
    state.context.value = context('mtgo')
    const mounted = mountFake(Host)
    open.push(mounted)
    await mounted.settle()
    expect(state.replaced).toEqual([])
    state.context.value = context('arena')
    await mounted.settle()
    expect(state.replaced).toEqual([{ name: 'dashboard' }])
  })

  it('stays on a page the new platform shows', async () => {
    state.route.value = { name: 'decks' }
    state.context.value = context('mtgo')
    const mounted = mountFake(Host)
    open.push(mounted)
    state.context.value = context('arena')
    await mounted.settle()
    expect(state.replaced).toEqual([])
  })
})
