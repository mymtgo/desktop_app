import { describe, expect, it, vi } from 'vitest'
import { h, ref } from 'vue'
import type { MostPickedView } from '@/bindings'
import { mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const fetched: Array<string | null> = []
vi.mock('@/lib/queries', async () => {
  const { ref: vueRef } = await import('vue')
  return {
    useDashboardPicks: (set: { value: string | null }, enabled: { value: boolean }) => {
      if (enabled.value) fetched.push(set.value)
      return { data: vueRef(undefined) }
    },
  }
})

// The select shows its model value as text, so the test can read the choice.
vi.mock('@/components/ui/select', async () => {
  const { h: vueH } = await import('vue')
  const Shown = {
    props: ['modelValue'],
    setup: (props: { modelValue: string }, { slots }: { slots: { default?: () => unknown } }) =>
      () => vueH('div', {}, [`selected:${props.modelValue}`, slots.default?.() as never]),
  }
  const Pass = { setup: (_: unknown, { slots }: { slots: { default?: () => unknown } }) => () => vueH('div', {}, slots.default?.() as never) }
  return { Select: Shown, SelectContent: Pass, SelectItem: Pass, SelectTrigger: Pass, SelectValue: Pass }
})

const { default: MostPickedCard } = await import('./MostPickedCard.vue')

function view(code: string, name: string): MostPickedView {
  return {
    sets: [
      { code: 'AAA', name: 'Set A' },
      { code: 'BBB', name: 'Set B' },
    ],
    picks: {
      setCode: code,
      setName: name,
      picks: [{ catalogId: 1, name: `Card of ${code}`, image: null, localImage: null, count: 3 }],
    },
  }
}

describe('the Most picked card', () => {
  it('follows a new view without fetching', async () => {
    const current = ref(view('AAA', 'Set A'))
    const mounted = mountFake({ render: () => h(MostPickedCard, { view: current.value }) })
    expect(textOf(mounted.root)).toContain('selected:AAA')
    expect(textOf(mounted.root)).toContain('Card of AAA')

    current.value = view('BBB', 'Set B')
    await mounted.settle()
    const text = textOf(mounted.root)
    expect(text).toContain('selected:BBB')
    expect(text).toContain('Card of BBB')
    expect(text).not.toContain('Card of AAA')
    expect(fetched).toEqual([])
    expect(mounted.warnings).toEqual([])
  })
})
