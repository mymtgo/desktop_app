import { beforeEach, describe, expect, it, vi } from 'vitest'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  calls: [] as string[],
  download: { status: 'ok', data: { added: 120, converted: 3, skipped: 2 } } as unknown,
  redetect: { status: 'ok', data: 41 } as unknown,
}))

vi.mock('@/bindings', () => ({
  commands: {
    downloadOfflineArchetypes: async () => {
      state.calls.push('download')
      return state.download
    },
    redetectOfflineMatches: async () => {
      state.calls.push('redetect')
      return state.redetect
    },
  },
}))

const { default: OfflineArchetypes } = await import('./OfflineArchetypes.vue')

beforeEach(() => {
  state.calls = []
  state.download = { status: 'ok', data: { added: 120, converted: 3, skipped: 2 } }
  state.redetect = { status: 'ok', data: 41 }
})

function buttons(root: ReturnType<typeof mountFake>['root']) {
  return findAll(root, (node) => node.tag === 'button')
}

function button(root: ReturnType<typeof mountFake>['root'], label: string) {
  return buttons(root).find((node) => textOf(node).includes(label))
}

describe('the offline archetypes panel', () => {
  it('shows nothing while online', () => {
    const mounted = mountFake(OfflineArchetypes, { offline: false, heldOffline: false })
    expect(mounted.warnings).toEqual([])
    expect(buttons(mounted.root)).toEqual([])
  })

  it('downloads, then offers detection, which queues the matches', async () => {
    const mounted = mountFake(OfflineArchetypes, { offline: true, heldOffline: false })
    expect(mounted.warnings).toEqual([])
    expect(button(mounted.root, 'Run detection')).toBeUndefined()
    ;(button(mounted.root, 'Download archetypes')!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['download'])
    const text = textOf(mounted.root)
    expect(text).toContain('Downloaded 123 archetypes. They are now Custom archetypes you can edit.')
    // The card reloads what is held after the download.
    mounted.setProps({ heldOffline: true })
    await mounted.settle()
    ;(button(mounted.root, 'Run detection against your matches now')!.props.onClick as () => void)()
    await mounted.settle()
    expect(state.calls).toEqual(['download', 'redetect'])
    expect(textOf(mounted.root)).toContain('Detection queued for 41 matches.')
  })

  it('waits for the offline download before offering detection', () => {
    const mounted = mountFake(OfflineArchetypes, { offline: true, heldOffline: false })
    expect(button(mounted.root, 'Run detection')).toBeUndefined()
    expect(button(mounted.root, 'Download archetypes')).toBeDefined()
  })

  it('offers detection when archetypes were downloaded offline before', () => {
    const mounted = mountFake(OfflineArchetypes, { offline: true, heldOffline: true })
    expect(button(mounted.root, 'Run detection against your matches now')).toBeDefined()
    expect(textOf(mounted.root)).toContain('Archetypes you set by hand are never changed.')
  })

  // PR #517 review: going online releases every held row, so coming back
  // offline in the same session starts over.
  it('forgets the last download after going online and back offline', async () => {
    const mounted = mountFake(OfflineArchetypes, { offline: true, heldOffline: false })
    ;(button(mounted.root, 'Download archetypes')!.props.onClick as () => void)()
    await mounted.settle()
    mounted.setProps({ offline: false, heldOffline: false })
    await mounted.settle()
    mounted.setProps({ offline: true })
    await mounted.settle()
    expect(button(mounted.root, 'Run detection')).toBeUndefined()
    expect(textOf(mounted.root)).not.toContain('Downloaded 123 archetypes')
  })

  it('shows a failed download', async () => {
    state.download = { status: 'error', error: { kind: 'refused', message: 'Could not download the archetypes: Offline' } }
    const mounted = mountFake(OfflineArchetypes, { offline: true, heldOffline: false })
    ;(button(mounted.root, 'Download archetypes')!.props.onClick as () => void)()
    await mounted.settle()
    expect(textOf(mounted.root)).toContain('Could not download the archetypes: Offline')
    expect(button(mounted.root, 'Run detection')).toBeUndefined()
  })
})
