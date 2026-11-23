import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  version: '0.45.0' as string | null,
  replaced: [] as unknown[],
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({
    replace: async (to: unknown) => {
      state.replaced.push(to)
    },
  }),
}))
vi.mock('@/lib/queries', () => ({
  useAppInfo: () => ({ data: ref(state.version === null ? undefined : { version: state.version }) }),
}))
// WhatsNewPageTest's markdown as a content component: a heading, a table and an image.
vi.mock('@/content/whats-new', async (importOriginal) => {
  const original = await importOriginal<typeof import('@/content/whats-new')>()
  const { defineComponent, h } = await import('vue')
  const page = defineComponent({
    setup: () => () =>
      h('div', [
        h('h1', 'New in 0.45'),
        h('table', [h('tbody', [h('tr', [h('td', '1'), h('td', '2')])])]),
        h('img', { src: '/assets/deck.png', alt: 'Deck page' }),
      ]),
  })
  const entries = [{ version: '0.45', component: page }]
  return {
    ...original,
    whatsNewEntries: entries,
    entryFor: (running: string) => original.entryFor(running, entries),
  }
})

const { default: WhatsNewPage } = await import('./WhatsNewPage.vue')

beforeEach(() => {
  state.version = '0.45.0'
  state.replaced = []
})

describe('the What\'s new page', () => {
  // WhatsNewPageTest "renders the markdown file with tables and images".
  it('renders the entry with its table and image in a max-w-3xl column', () => {
    const mounted = mountFake(WhatsNewPage)
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toContain('New in 0.45')
    expect(findAll(mounted.root, (node) => node.tag === 'table')).toHaveLength(1)
    const [image] = findAll(mounted.root, (node) => node.tag === 'img')
    expect(image!.props).toMatchObject({ src: '/assets/deck.png', alt: 'Deck page' })
    expect(findAll(mounted.root, (node) => hasClass(node, 'max-w-3xl'))).toHaveLength(1)
    expect(findAll(mounted.root, (node) => hasClass(node, 'markdown-content'))).toHaveLength(1)
    expect(state.replaced).toEqual([])
  })

  // "404s when the file is missing" / "is blank": v1 goes to the dashboard.
  it('goes to the dashboard when there is no entry for the running version', async () => {
    state.version = '0.44.0'
    const mounted = mountFake(WhatsNewPage)
    await Promise.resolve()
    expect(findAll(mounted.root, (node) => node.tag === 'table')).toHaveLength(0)
    expect(state.replaced).toEqual([{ name: 'dashboard' }])
  })

  it('waits for the running version before deciding', () => {
    state.version = null
    mountFake(WhatsNewPage)
    expect(state.replaced).toEqual([])
  })
})
