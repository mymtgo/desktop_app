import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mountFake } from '@/replay/replayTestHarness'
import { useTauriEvents, type TauriEvents } from './useTauriEvents'

/** A listener whose registration resolves only when the test says so. */
function pendingListener() {
  const stop = vi.fn()
  let register: () => void = () => {}
  const listen = () =>
    new Promise<() => void>((resolve) => {
      register = () => resolve(stop)
    })
  return { stop, listen, register: () => register() }
}

function mountWith(listen: () => Promise<() => void>[]) {
  let events: TauriEvents | null = null
  const component = defineComponent({
    setup() {
      events = useTauriEvents(listen)
      return () => h('div')
    },
  })
  const mounted = mountFake(component)
  return { mounted, events: events! }
}

describe('useTauriEvents', () => {
  it('removes its listeners on unmount', async () => {
    const first = pendingListener()
    const second = pendingListener()
    const { mounted, events } = mountWith(() => [first.listen(), second.listen()])

    first.register()
    second.register()
    expect(await events.ready).toBe(true)
    expect(events.isMounted()).toBe(true)
    expect(first.stop).not.toHaveBeenCalled()

    mounted.unmount()
    expect(first.stop).toHaveBeenCalledTimes(1)
    expect(second.stop).toHaveBeenCalledTimes(1)
    expect(events.isMounted()).toBe(false)
  })

  it('removes listeners that register after the component unmounted', async () => {
    const listener = pendingListener()
    const { mounted, events } = mountWith(() => [listener.listen()])

    mounted.unmount()
    listener.register()

    expect(await events.ready).toBe(false)
    expect(listener.stop).toHaveBeenCalledTimes(1)
  })
})
