import { describe, expect, it, vi } from 'vitest'
import { TEST_BUNDLES } from '@/lib/wrapped/testBundles'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: DashboardWrappedCard } = await import('./DashboardWrappedCard.vue')

describe('the dashboard Wrapped card', () => {
  it('links to /wrapped and gives none of the numbers away', () => {
    const mounted = mountFake(DashboardWrappedCard, { bundle: TEST_BUNDLES.grinder })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('Your 2026 in Magic is ready')
    expect(text).not.toContain('1,284')
    expect(text).not.toContain('Boros Energy')
    expect(text).not.toContain('troph')
    const links = findAll(mounted.root, (node) => node.tag === 'a' && JSON.stringify(node.props.href ?? null).includes('wrapped'))
    expect(links.length).toBeGreaterThan(0)
  })

  it('clicking the close button emits dismiss', () => {
    let dismissed = 0
    const mounted = mountFake(DashboardWrappedCard, { bundle: TEST_BUNDLES.light, onDismiss: () => (dismissed += 1) })
    const close = findAll(mounted.root, (node) => node.tag === 'button' && node.props['aria-label'] === 'Dismiss Wrapped')
    expect(close).toHaveLength(1)
    ;(close[0].props.onClick as () => void)()
    expect(dismissed).toBe(1)
  })
})
