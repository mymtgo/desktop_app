import { describe, expect, it, vi } from 'vitest'
import { mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('@/composables/useLeagueManage', async () => {
  const { ref } = await import('vue')
  return { useLeagueManage: () => ({ saving: ref(false), setStatus: async () => true }) }
})
vi.mock('@/components/ui/dialog', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = (tag: string) =>
    defineComponent({ inheritAttrs: true, setup: (_, { slots }) => () => h(tag, slots.default?.()) })
  return {
    Dialog: plain('dialog'),
    DialogContent: plain('dialog-content'),
    DialogFooter: plain('dialog-footer'),
    DialogHeader: plain('dialog-header'),
    DialogTitle: plain('dialog-title'),
  }
})

const { default: DropLeagueDialog } = await import('./DropLeagueDialog.vue')

describe('the Drop dialog', () => {
  it('says an MTGO league is not dropped in the MTGO client', () => {
    const mounted = mountFake(DropLeagueDialog, {})
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Drop league?')
    expect(text).toContain('in the MTGO client')
  })

  it('words an Arena run for Arena', () => {
    const mounted = mountFake(DropLeagueDialog, { platform: 'arena' })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Drop this run?')
    expect(text).toContain('in Arena')
    expect(text).not.toContain('MTGO')
    expect(text).not.toContain('league')
  })
})
