import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { FolderStatuses, Settings, StatusView } from '@/bindings'
import { type FakeNode, findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  settings: null as unknown as import('vue').Ref<Settings>,
  statuses: null as unknown as import('vue').Ref<FolderStatuses>,
  saved: [] as Settings[],
  browsed: [] as (string | null)[],
  pick: null as string | null,
  invalidated: 0,
  hold: null as Promise<void> | null,
  platform: 'mtgo' as 'mtgo' | 'arena',
  arena: true,
  os: 'windows',
}))

vi.mock('@/bindings', () => ({
  commands: {
    chooseFolder: async (start: string | null) => {
      state.browsed.push(start)
      if (state.hold) await state.hold
      return state.pick
    },
  },
}))
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  return {
    useSettings: () => ({ data: state.settings }),
    useStatus: () => ({
      data: ref({
        searched: ['C:/Users/Q/AppData/Local/Apps/2.0'],
        files: [{ path: 'C:/x/mtgo.log', size: 1, cursor: 1, lastLiveGrowthMs: null, badTimezone: false }],
        arenaSearched: ['C:/Users/Q/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log'],
        arenaFiles: [],
      } as unknown as StatusView),
    }),
    useIntegrationsQuery: () => ({ data: ref({ arena: state.arena }) }),
    useFolderStatuses: () => ({ data: state.statuses }),
    useAppInfo: () => ({ data: ref({ os: state.os }) }),
  }
})
vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({
    getQueryData: () => state.settings.value,
    invalidateQueries: async () => {
      state.invalidated += 1
    },
  }),
}))
vi.mock('@/composables/usePlatformContext', async () => {
  const { computed } = await import('vue')
  return { usePlatformContext: () => ({ platform: computed(() => state.platform) }) }
})
vi.mock('@/composables/useSaveSettings', async () => {
  const { ref } = await import('vue')
  return {
    useSaveSettings: () => ({
      saving: ref(false),
      save: async (next: Settings) => {
        state.saved.push(next)
        return true
      },
    }),
  }
})
vi.mock('@/components/ui/input', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Input: defineComponent({
      props: { modelValue: String },
      emits: ['update:modelValue'],
      setup: (props, { emit }) => () =>
        h('input', { value: props.modelValue, onInput: (value: string) => emit('update:modelValue', value) }),
    }),
  }
})

const { default: FilePathsCard } = await import('./FilePathsCard.vue')

function settings(overrides: Partial<Settings> = {}): Settings {
  return { logFolder: null, mtgoDataFolder: null, watcherActive: true, closeToTray: true, ...overrides } as Settings
}

beforeEach(() => {
  state.settings = ref(settings())
  state.statuses = ref({ log: null, data: null })
  state.saved = []
  state.arena = true
  state.browsed = []
  state.pick = null
  state.invalidated = 0
  state.hold = null
  state.platform = 'mtgo'
  state.os = 'windows'
})

function mount() {
  const mounted = mountFake(FilePathsCard)
  expect(mounted.warnings).toEqual([])
  return mounted
}

const inputs = (root: FakeNode) => findAll(root, (node) => node.tag === 'input')
const buttons = (root: FakeNode, label: string) =>
  findAll(root, (node) => node.tag === 'button' && textOf(node).trim() === label)

describe('the File Paths card', () => {
  it('shows where Player.log is looked for in the Arena context, with no MTGO folders', async () => {
    state.platform = 'arena'
    const mounted = mount()
    const { root } = mounted
    const text = textOf(root)
    const where = 'C:/Users/Q/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log'
    expect(text).toContain('Arena Log File')
    expect(text).toContain("Can't find Player.log. Checked 1 possible location.")
    expect(text).not.toContain(where)
    ;(buttons(root, 'View locations')[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(textOf(mounted.root)).toContain(where)
    expect(buttons(mounted.root, 'Hide locations')).toHaveLength(1)
    expect(text).not.toContain('Log File Directory')
    expect(text).not.toContain('Game Data Directory')
    expect(inputs(root)).toHaveLength(0)
  })

  it('hides the Arena block while Arena is switched off', () => {
    state.platform = 'arena'
    state.arena = false
    const { root } = mount()
    expect(textOf(root)).not.toContain('Arena Log File')
  })

  it('shows the folder dialog as busy, not as Saving', async () => {
    let close = () => {}
    state.hold = new Promise<void>((resolve) => (close = resolve))
    const mounted = mount()
    const browse = buttons(mounted.root, 'Browse')[0]!
    ;(browse.props.onClick as () => void)()
    await mounted.settle()
    expect(textOf(mounted.root)).not.toContain('Saving...')
    expect(buttons(mounted.root, 'Browse')[0]!.props.disabled).toBe(true)
    close()
    await mounted.settle()
    expect(buttons(mounted.root, 'Browse')[0]!.props.disabled).toBe(false)
  })

  it('has 0.x title, description, labels, hints and buttons', () => {
    const text = textOf(mount().root)
    expect(text).toContain('File Paths')
    expect(text).toContain(
      'Where to look for MTGO log files and game data. Defaults are set automatically for standard installs.',
    )
    expect(text).toContain('Log File Directory')
    expect(text).toContain('Contains mtgo.log files')
    expect(text).toContain('Game Data Directory')
    expect(text).toContain('Contains Match_GameLog_* and deck XML files')
    const { root } = mount()
    expect(buttons(root, 'Browse')).toHaveLength(2)
    expect(buttons(root, 'Save')).toHaveLength(2)
  })

  it('tells a Mac user about Wine wrappers and virtual machines', () => {
    expect(textOf(mount().root)).not.toContain('Windows virtual machine')
    state.os = 'macos'
    const text = textOf(mount().root)
    expect(text).toContain('MTGO under CrossOver, Whisky or another Wine wrapper is found automatically.')
    expect(text).toContain('In a Windows virtual machine, share its MTGO folder with your Mac and choose it below.')
  })

  it('shows where the log was found, not every place searched, and no Use automatic button', () => {
    const text = textOf(mount().root)
    expect(text).toContain('Found mtgo.log')
    expect(text).toContain('C:/x/mtgo.log')
    expect(text).not.toContain('C:/Users/Q/AppData/Local/Apps/2.0')
    expect(text).not.toContain('Use automatic')
  })

  it('shows the 0.x status messages of chosen folders, with Use automatic', () => {
    state.settings.value = settings({ logFolder: 'D:/Q/logs', mtgoDataFolder: 'D:/Q/data' })
    state.statuses.value = {
      log: { valid: true, fileCount: 2, message: 'Found 2 log files.' },
      data: { valid: false, fileCount: 0, message: 'No Match_GameLog files found in this directory.' },
    }
    const { root } = mount()
    const text = textOf(root)
    expect(text).toContain('Found 2 log files.')
    expect(text).toContain('No Match_GameLog files found in this directory.')
    expect(buttons(root, 'Use automatic')).toHaveLength(2)
    expect(inputs(root).map((node) => node.props.value)).toEqual(['D:/Q/logs', 'D:/Q/data'])
  })

  it('disables Save while unchanged and saves on Save or Enter once changed', async () => {
    const mounted = mount()
    const save = () => buttons(mounted.root, 'Save')[0]!
    expect(save().props.disabled).toBe(true)
    ;(inputs(mounted.root)[0]!.props.onInput as (value: string) => void)('D:/Q/logs')
    await mounted.settle()
    expect(save().props.disabled).toBe(false)
    expect(buttons(mounted.root, 'Save')[1]!.props.disabled).toBe(true)
    ;(save().props.onClick as () => void)()
    await mounted.settle()
    expect(state.saved.at(-1)?.logFolder).toBe('D:/Q/logs')
    expect(state.saved.at(-1)?.mtgoDataFolder).toBeNull()
    expect(state.invalidated).toBe(1)

    ;(inputs(mounted.root)[1]!.props.onInput as (value: string) => void)('D:/Q/data')
    ;(inputs(mounted.root)[1]!.props.onKeydown as (event: { key: string }) => void)({ key: 'Enter' })
    await mounted.settle()
    expect(state.saved.at(-1)?.mtgoDataFolder).toBe('D:/Q/data')
  })

  it('saves a folder picked with Browse, opened at the typed folder', async () => {
    state.settings.value = settings({ mtgoDataFolder: 'D:/Q/data' })
    state.pick = 'E:/Q/picked'
    const mounted = mount()
    ;(buttons(mounted.root, 'Browse')[1]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.browsed).toEqual(['D:/Q/data'])
    expect(state.saved.at(-1)?.mtgoDataFolder).toBe('E:/Q/picked')
  })

  it('saves nothing when Browse is cancelled', async () => {
    const mounted = mount()
    ;(buttons(mounted.root, 'Browse')[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.browsed).toEqual([null])
    expect(state.saved).toEqual([])
  })

  it('goes back to automatic with Use automatic', async () => {
    state.settings.value = settings({ logFolder: 'D:/Q/logs' })
    const mounted = mount()
    ;(buttons(mounted.root, 'Use automatic')[0]!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.saved.at(-1)?.logFolder).toBeNull()
  })
})
