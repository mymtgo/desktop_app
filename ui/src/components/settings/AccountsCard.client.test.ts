import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { AccountsView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  view: { accounts: [], current: null } as AccountsView,
  sent: [] as Array<[string, boolean]>,
  stored: null as AccountsView | null,
}))

vi.mock('@/bindings', () => ({
  commands: {
    setAccountTracked: async (username: string, tracked: boolean) => {
      state.sent.push([username, tracked])
      return { status: 'ok', data: state.view }
    },
  },
}))
vi.mock('@/lib/queries', () => ({ useAccounts: () => ({ data: ref(state.view) }) }))
vi.mock('@tanstack/vue-query', () => ({
  useQueryClient: () => ({
    setQueryData: (_key: unknown, value: AccountsView) => {
      state.stored = value
    },
  }),
}))
// A plain switch: reka's needs a DOM. It keeps its listener as a prop.
vi.mock('@/components/ui/switch', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Switch: defineComponent({
      props: { modelValue: Boolean, disabled: Boolean },
      emits: ['update:modelValue'],
      setup: (props, { emit }) => () =>
        h('switch', {
          modelValue: props.modelValue,
          disabled: props.disabled,
          'onUpdate:modelValue': (value: boolean) => emit('update:modelValue', value),
        }),
    }),
  }
})
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ add: () => {} }) }))

const { default: AccountsCard } = await import('./AccountsCard.vue')

beforeEach(() => {
  state.view = {
    accounts: [
      { id: 1, username: 'Qplayer', label: 'Qplayer', tracked: true, active: true },
      { id: 2, username: 'Qsecond', label: 'Qsecond', tracked: false, active: false },
    ],
    current: 'Qplayer',
  }
  state.sent = []
  state.stored = null
})

function mount() {
  const mounted = mountFake(AccountsCard)
  expect(mounted.warnings).toEqual([])
  return mounted
}

describe('the Accounts card', () => {
  it('has the 0.x strings, the Active badge and each sub-text', () => {
    const mounted = mount()
    const text = textOf(mounted.root)
    expect(text).toContain('Accounts')
    expect(text).toContain('Toggle tracking to control which accounts record match data.')
    const rows = findAll(mounted.root, (node) => node.props['data-account'] !== undefined)
    const texts = rows.map((row) => textOf(row))
    expect(texts).toHaveLength(2)
    expect(texts[0]).toContain('Qplayer')
    expect(texts[0]).toContain('Active')
    expect(texts[0]).toContain('Recording matches')
    expect(texts[1]).toContain('Qsecond')
    expect(texts[1]).not.toContain('Active')
    expect(texts[1]).toContain('Not recording matches')
  })

  it('toggles tracking per account', async () => {
    const mounted = mount()
    const switches = findAll(mounted.root, (node) => node.tag === 'switch')
    expect(switches.map((node) => node.props.modelValue)).toEqual([true, false])
    ;(switches[1]!.props['onUpdate:modelValue'] as (value: boolean) => void)(true)
    await mounted.settle()
    expect(state.sent).toEqual([['Qsecond', true]])
    expect(state.stored).toEqual(state.view)
  })

  it('is hidden without accounts', () => {
    state.view = { accounts: [], current: null }
    const mounted = mount()
    expect(textOf(mounted.root)).not.toContain('Accounts')
  })
})
