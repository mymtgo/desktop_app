import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { Settings, StatusView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  active: true,
  found: true,
  sent: [] as boolean[],
  stored: null as Settings | null,
}))

vi.mock('@/bindings', () => ({
  commands: {
    setWatcherActive: async (active: boolean) => {
      state.sent.push(active)
      return { status: 'ok', data: { watcherActive: active } }
    },
  },
}))
vi.mock('@/lib/queries', () => ({
  useSettings: () => ({ data: ref({ watcherActive: state.active } as Settings) }),
  useStatus: () => ({
    data: ref({
      files: state.found ? [{ path: 'C:/x/mtgo.log' }] : [],
      arenaFiles: [],
    } as unknown as StatusView),
  }),
}))
vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({
    setQueryData: (_key: unknown, value: Settings) => {
      state.stored = value
    },
  }),
}))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))

const { default: WatcherCard } = await import('./WatcherCard.vue')

beforeEach(() => {
  state.active = true
  state.found = true
  state.sent = []
  state.stored = null
})

function mount() {
  const mounted = mountFake(WatcherCard)
  expect(mounted.warnings).toEqual([])
  const button = () => findAll(mounted.root, (node) => node.tag === 'button')[0]!
  return { mounted, text: textOf(mounted.root), button }
}

describe('the Watcher card', () => {
  it('has 0.x strings', () => {
    const { text } = mount()
    expect(text).toContain('Watcher')
    expect(text).toContain('Control the file system watcher that monitors log files and triggers ingestion.')
    expect(text).toContain('File Watcher')
    expect(text).toContain('Monitors log files and triggers ingestion automatically.')
  })

  it('says Running and offers Stop when on with a log found', async () => {
    const { mounted, text, button } = mount()
    expect(text).toContain('Running')
    expect(textOf(button()).trim()).toBe('Stop')
    expect(button().props.disabled).toBe(false)
    expect(text).not.toContain('File paths must be valid to enable the watcher.')
    ;(button().props.onClick as () => void)()
    await mounted.settle()
    expect(state.sent).toEqual([false])
    expect(state.stored?.watcherActive).toBe(false)
  })

  it('says Stopped and offers Start when off', async () => {
    state.active = false
    const { mounted, text, button } = mount()
    expect(text).toContain('Stopped')
    expect(textOf(button()).trim()).toBe('Start')
    ;(button().props.onClick as () => void)()
    await mounted.settle()
    expect(state.sent).toEqual([true])
  })

  it('disables Start while no log is found, with the 0.x note', () => {
    state.active = false
    state.found = false
    const { text, button } = mount()
    expect(text).toContain('Stopped')
    expect(text).toContain('File paths must be valid to enable the watcher.')
    expect(textOf(button()).trim()).toBe('Start')
    expect(button().props.disabled).toBe(true)
  })
})
