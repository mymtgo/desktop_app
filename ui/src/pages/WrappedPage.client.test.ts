import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import type { WrappedBundle } from '@/bindings'
import { TEST_BUNDLES } from '@/lib/wrapped/testBundles'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

const state = { bundle: TEST_BUNDLES.light as WrappedBundle }

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub, useRouter: () => ({ push: async () => {} }) }
})
vi.mock('@/lib/wrapped/useWrapped', () => ({
  useWrapped: () => ({ bundle: ref(state.bundle) }),
}))

const { default: WrappedPage } = await import('./WrappedPage.vue')

function firstYearNotes(root: ReturnType<typeof mountFake>['root']) {
  return findAll(root, (node) => hasClass(node, 'wrapped-first-year'))
}

describe('the Wrapped page', () => {
  it('a first Wrapped says so once, at the foot of the page', () => {
    state.bundle = { ...TEST_BUNDLES.light, firstYear: true }
    const mounted = mountFake(WrappedPage, {})
    const notes = firstYearNotes(mounted.root)
    expect(notes).toHaveLength(1)
    expect(textOf(notes[0])).toContain('first MyMTGO Wrapped')
  })

  it('a later Wrapped has no first-year note', () => {
    state.bundle = { ...TEST_BUNDLES.light, firstYear: false }
    const mounted = mountFake(WrappedPage, {})
    expect(firstYearNotes(mounted.root)).toEqual([])
    expect(textOf(mounted.root)).not.toContain('first MyMTGO Wrapped')
  })
})
