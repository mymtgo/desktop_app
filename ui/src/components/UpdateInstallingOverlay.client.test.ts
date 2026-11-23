import { describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'
import { mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({ installing: false }))

vi.mock('@/composables/useInstallUpdate', () => ({
  useInstallUpdate: () => ({ installing: ref(state.installing), install: () => {} }),
}))

const { default: UpdateInstallingOverlay } = await import('./UpdateInstallingOverlay.vue')

function text(installing: boolean) {
  state.installing = installing
  const mounted = mountFake(UpdateInstallingOverlay)
  expect(mounted.warnings).toEqual([])
  return textOf(mounted.root)
}

describe('the update installing overlay', () => {
  it("says 0.x's text while the update installs", () => {
    const shown = text(true)
    expect(shown).toContain('Installing update...')
    expect(shown).toContain(
      "The app will close and restart automatically. This may take a few minutes, so please don't reopen the app manually.",
    )
  })

  it('is hidden otherwise', () => {
    expect(text(false)).toBe('')
  })
})
