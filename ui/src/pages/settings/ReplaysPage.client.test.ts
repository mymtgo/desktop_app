import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { AppInfo, RecorderStatus, RecorderStatusView } from '@/bindings'
import { findAll, mountFake, textOf, type FakeNode } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  os: 'windows',
  view: null as unknown,
  calls: [] as string[],
  // Commands that answer with an error, or whose invoke rejects.
  errors: {} as Record<string, string>,
  rejects: {} as Record<string, boolean>,
}))

function viewWith(status: RecorderStatus, needsAcceptance = false): RecorderStatusView {
  return { status, folder: 'C:\\Users\\PlayerA\\Documents\\MtgReplay', imported: 7, needsAcceptance, version: '0.3.1' }
}

vi.mock('@/bindings', () => {
  const reply = (name: string) => async () => {
    state.calls.push(name)
    if (state.rejects[name]) throw new Error(`${name} rejected`)
    if (state.errors[name]) return { status: 'error', error: state.errors[name] }
    return { status: 'ok', data: state.view }
  }
  return {
    commands: {
      recorderStatus: reply('recorderStatus'),
      acceptRecorderNotice: reply('acceptRecorderNotice'),
      setRecorderEnabled: async (on: boolean) => {
        state.calls.push(`setRecorderEnabled:${on}`)
        if (state.rejects.setRecorderEnabled) throw new Error('setRecorderEnabled rejected')
        return { status: 'ok', data: state.view }
      },
      retryRecorder: reply('retryRecorder'),
      openRecorderFolder: reply('openRecorderFolder'),
      openExternal: async (url: string) => {
        state.calls.push(`openExternal:${url}`)
        return { status: 'ok', data: null }
      },
    },
  }
})
vi.mock('@/lib/queries', () => ({
  useAppInfo: () => ({ data: ref({ os: state.os } as AppInfo) }),
}))
// The fake renderer has no DOM: the library widgets become plain elements.
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: ['modelValue', 'disabled'],
      emits: ['update:modelValue'],
      setup: (p, { emit }) => () =>
        h('sw', { on: p.modelValue, disabled: p.disabled, onClick: () => emit('update:modelValue', !p.modelValue) }),
    }),
  }
})
vi.mock('@/components/ui/checkbox', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Checkbox: defineComponent({
      props: ['modelValue'],
      emits: ['update:modelValue'],
      setup: (p, { emit }) => () => h('cb', { checked: p.modelValue, onClick: () => emit('update:modelValue', !p.modelValue) }),
    }),
  }
})
vi.mock('@/components/ui/button', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Button: defineComponent({
      props: ['disabled'],
      setup: (p, { slots }) => () => h('btn', { disabled: p.disabled }, slots.default?.()),
    }),
  }
})
vi.mock('@/components/ui/label', async () => {
  const { defineComponent, h } = await import('vue')
  return { Label: defineComponent({ setup: (_p, { slots }) => () => h('label', {}, slots.default?.()) }) }
})
vi.mock('@/components/ui/dialog', async () => {
  const { defineComponent, h } = await import('vue')
  const box = (tag: string) => defineComponent({ setup: (_p, { slots }) => () => h(tag, {}, slots.default?.()) })
  return {
    Dialog: defineComponent({
      props: ['open'],
      setup: (p, { slots }) => () => (p.open ? h('dialog-box', {}, slots.default?.()) : null),
    }),
    DialogContent: box('dc'),
    DialogDescription: box('dd'),
    DialogFooter: box('df'),
    DialogHeader: box('dh'),
    DialogTitle: box('dt'),
  }
})

const { default: ReplaysPage } = await import('./ReplaysPage.vue')

function button(root: FakeNode, label: string): FakeNode {
  const found = findAll(root, (n) => n.tag === 'btn' && textOf(n).trim() === label)
  expect(found, `button ${label}`).toHaveLength(1)
  return found[0]
}

function pressSwitch(root: FakeNode) {
  const sw = findAll(root, (n) => n.tag === 'sw')[0]
  ;(sw.props.onClick as () => void)()
}

async function mountPage(status: RecorderStatus, needsAcceptance = false) {
  state.view = viewWith(status, needsAcceptance)
  const mounted = mountFake(ReplaysPage)
  await mounted.settle()
  return mounted
}

beforeEach(() => {
  state.os = 'windows'
  state.calls = []
  state.errors = {}
  state.rejects = {}
})

describe('the Replays settings page', () => {
  it('the_switch_starts_off', async () => {
    const mounted = await mountPage('off', true)
    const sw = findAll(mounted.root, (n) => n.tag === 'sw')[0]
    expect(sw.props.on).toBe(false)
    expect(textOf(mounted.root)).toContain('Off')
    expect(textOf(mounted.root)).toContain('Replays imported: 7')
    expect(textOf(mounted.root)).toContain('MtgReplay')
    mounted.unmount()
  })

  it('turning_it_on_opens_the_notice_and_downloads_nothing', async () => {
    const mounted = await mountPage('off', true)
    state.calls = []
    pressSwitch(mounted.root)
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('I understand and accept the risk')
    expect(state.calls).toEqual([])
    mounted.unmount()
  })

  it('turning_it_on_when_already_accepted_enables_it_without_the_notice', async () => {
    const mounted = await mountPage('off', false)
    pressSwitch(mounted.root)
    await mounted.settle()
    expect(state.calls).toContain('setRecorderEnabled:true')
    expect(textOf(mounted.root)).not.toContain('I understand and accept the risk')
    mounted.unmount()
  })

  it('turning_it_off_calls_set_recorder_enabled_false', async () => {
    const mounted = await mountPage('waiting')
    pressSwitch(mounted.root)
    await mounted.settle()
    expect(state.calls).toContain('setRecorderEnabled:false')
    mounted.unmount()
  })

  it('download_is_disabled_until_the_box_is_ticked', async () => {
    const mounted = await mountPage('off', true)
    pressSwitch(mounted.root)
    await mounted.settle()
    expect(button(mounted.root, 'Download and enable').props.disabled).toBe(true)
    ;(findAll(mounted.root, (n) => n.tag === 'cb')[0].props.onClick as () => void)()
    await mounted.settle()
    expect(button(mounted.root, 'Download and enable').props.disabled).toBeFalsy()
    mounted.unmount()
  })

  it('accepting_calls_accept_recorder_notice', async () => {
    const mounted = await mountPage('off', true)
    pressSwitch(mounted.root)
    await mounted.settle()
    ;(findAll(mounted.root, (n) => n.tag === 'cb')[0].props.onClick as () => void)()
    await mounted.settle()
    state.calls = []
    ;(button(mounted.root, 'Download and enable').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['acceptRecorderNotice'])
    mounted.unmount()
  })

  it('cancel_closes_the_notice_and_calls_nothing', async () => {
    const mounted = await mountPage('off', true)
    pressSwitch(mounted.root)
    await mounted.settle()
    state.calls = []
    ;(button(mounted.root, 'Cancel').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual([])
    expect(textOf(mounted.root)).not.toContain('I understand and accept the risk')
    mounted.unmount()
  })

  it('each_status_shows_its_text', async () => {
    const cases: [RecorderStatus, string, string | null][] = [
      ['off', 'Off', null],
      ['downloading', 'Downloading the recorder', null],
      ['noBuild', 'No recorder build in this version yet', null],
      [{ failed: { reason: 'no network' } }, 'Failed: no network', 'Retry'],
      ['blockedByAntivirus', 'Blocked by antivirus: allow mtgr-record in Windows Security, then Retry', 'Retry'],
      ['waiting', 'Waiting for MTGO', null],
      ['ready', 'Connected to MTGO, waiting for a match', null],
      ['recording', 'Recording', null],
      ['anotherRunning', 'Another recorder is running', null],
      ['mtgoNotSupported', 'This MTGO version is not supported yet; retrying every minute', 'Retry'],
      ['stoppedAfterCrashes', 'Stopped after repeated crashes', 'Start again'],
    ]
    for (const [status, text, actionLabel] of cases) {
      const mounted = await mountPage(status)
      const line = findAll(mounted.root, (n) => n.props['data-testid'] === 'recorder-status')[0]
      expect(textOf(line)).toBe(text)
      const labels = findAll(mounted.root, (n) => n.tag === 'btn').map((n) => textOf(n).trim())
      for (const candidate of ['Retry', 'Start again']) {
        expect(labels.includes(candidate)).toBe(candidate === actionLabel)
      }
      mounted.unmount()
    }
  })

  it('no_build_disables_the_switch_and_never_opens_the_notice', async () => {
    const mounted = await mountPage('noBuild', true)
    const sw = findAll(mounted.root, (n) => n.tag === 'sw')[0]
    expect(sw.props.disabled).toBe(true)
    expect(sw.props.on).toBe(false)
    pressSwitch(mounted.root)
    await mounted.settle()
    expect(textOf(mounted.root)).not.toContain('I understand and accept the risk')
    mounted.unmount()
  })

  it('mtgo_not_supported_retry_calls_retry_recorder', async () => {
    const mounted = await mountPage('mtgoNotSupported')
    state.calls = []
    ;(button(mounted.root, 'Retry').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['retryRecorder'])
    mounted.unmount()
  })

  it('a_failed_accept_keeps_the_notice_open_and_shows_the_error_in_it', async () => {
    const mounted = await mountPage('off', true)
    pressSwitch(mounted.root)
    await mounted.settle()
    ;(findAll(mounted.root, (n) => n.tag === 'cb')[0].props.onClick as () => void)()
    await mounted.settle()
    state.errors.acceptRecorderNotice = 'The settings could not be saved'
    ;(button(mounted.root, 'Download and enable').props.onClick as () => void)()
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('I understand and accept the risk')
    const shown = findAll(mounted.root, (n) => n.props['data-testid'] === 'recorder-notice-error')
    expect(shown).toHaveLength(1)
    expect(textOf(shown[0])).toBe('The settings could not be saved')
    mounted.unmount()
  })

  it('a_rejected_invoke_shows_a_problem_and_a_rejected_poll_is_ignored', async () => {
    const mounted = await mountPage('recording')
    state.rejects.setRecorderEnabled = true
    pressSwitch(mounted.root)
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('setRecorderEnabled rejected')
    expect(findAll(mounted.root, (n) => n.tag === 'sw')[0].props.disabled).toBe(false)
    vi.useFakeTimers()
    try {
      state.rejects.recorderStatus = true
      await vi.advanceTimersByTimeAsync(5_000)
      expect(state.calls).toContain('recorderStatus')
    } finally {
      vi.useRealTimers()
    }
    mounted.unmount()
  })

  it('the_folder_card_links_the_recorder_notices_by_version', async () => {
    const mounted = await mountPage('ready')
    state.calls = []
    ;(button(mounted.root, 'Recorder NOTICE').props.onClick as () => void)()
    ;(button(mounted.root, 'Recorder third-party notices').props.onClick as () => void)()
    expect(state.calls).toEqual([
      'openExternal:https://releases.mymtgo.com/mtgr-record/0.3.1/NOTICE',
      'openExternal:https://releases.mymtgo.com/mtgr-record/0.3.1/THIRD-PARTY-NOTICES.md',
    ])
    mounted.unmount()
  })

  it('the_action_button_calls_retry_recorder', async () => {
    const mounted = await mountPage('stoppedAfterCrashes')
    state.calls = []
    ;(button(mounted.root, 'Start again').props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['retryRecorder'])
    mounted.unmount()
  })

  it('linux_shows_windows_only_and_a_disabled_switch', async () => {
    state.os = 'linux'
    const mounted = await mountPage('off', true)
    expect(textOf(mounted.root)).toContain('Replays are recorded on Windows only')
    expect(findAll(mounted.root, (n) => n.tag === 'sw')[0].props.disabled).toBe(true)
    mounted.unmount()
  })

  it('opens the folder from its button', async () => {
    const mounted = await mountPage('off')
    state.calls = []
    ;(button(mounted.root, 'Open folder').props.onClick as () => void)()
    expect(state.calls).toEqual(['openRecorderFolder'])
    mounted.unmount()
  })

  it('polls the status every five seconds and stops when unmounted', async () => {
    vi.useFakeTimers()
    try {
      const mounted = await mountPage('off')
      const before = state.calls.filter((c) => c === 'recorderStatus').length
      await vi.advanceTimersByTimeAsync(10_000)
      expect(state.calls.filter((c) => c === 'recorderStatus').length).toBe(before + 2)
      mounted.unmount()
      await vi.advanceTimersByTimeAsync(10_000)
      expect(state.calls.filter((c) => c === 'recorderStatus').length).toBe(before + 2)
    } finally {
      vi.useRealTimers()
    }
  })

  it('the_notice_says_it_loads_code_into_mtgo', async () => {
    const mounted = await mountPage('off', true)
    pressSwitch(mounted.root)
    await mounted.settle()
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    expect(text).toContain('loads code into the MTGO process')
    expect(text).toContain('freeze or crash')
    expect(text).toContain('You are downloading and running this at your own risk.')
    expect(text).not.toContain('sends none of your data')
    mounted.unmount()
  })

  it('the notice links open through openExternal, the recorder notices by version', async () => {
    const mounted = await mountPage('off', true)
    pressSwitch(mounted.root)
    await mounted.settle()
    state.calls = []
    for (const label of ['Daybreak MTGO EULA', 'MTGOSDK', 'MTGOSDK licence', 'Recorder notice', 'Third-party notices']) {
      ;(button(mounted.root, label).props.onClick as () => void)()
    }
    expect(state.calls).toEqual([
      'openExternal:https://www.mtgo.com/eula',
      'openExternal:https://github.com/videre-project/MTGOSDK',
      'openExternal:https://github.com/videre-project/MTGOSDK/blob/main/LICENSE',
      'openExternal:https://releases.mymtgo.com/mtgr-record/0.3.1/NOTICE',
      'openExternal:https://releases.mymtgo.com/mtgr-record/0.3.1/THIRD-PARTY-NOTICES.md',
    ])
    mounted.unmount()
  })
})
