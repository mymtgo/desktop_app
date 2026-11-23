import { describe, expect, it } from 'vitest'
import { INSTALL_MARKER } from '../page/hook'
import { activeTabNeedsReload } from './tabHook'

describe('activeTabNeedsReload', () => {
  it('is_true_when_the_active_endstep_tab_has_no_hook', async () => {
    const asked: unknown[] = []
    const needs = await activeTabNeedsReload({
      activeEndstepTab: async () => 7,
      hasHook: async (tabId, marker) => {
        asked.push([tabId, marker])
        return false
      },
    })
    expect(needs).toBe(true)
    expect(asked).toEqual([[7, INSTALL_MARKER]])
  })

  it('is_false_when_the_tab_has_the_hook', async () => {
    expect(await activeTabNeedsReload({ activeEndstepTab: async () => 7, hasHook: async () => true })).toBe(false)
  })

  it('is_false_when_the_active_tab_is_not_on_endstep', async () => {
    let probed = false
    const needs = await activeTabNeedsReload({
      activeEndstepTab: async () => null,
      hasHook: async () => {
        probed = true
        return false
      },
    })
    expect(needs).toBe(false)
    expect(probed).toBe(false)
  })

  it('is_false_when_the_probe_fails', async () => {
    expect(
      await activeTabNeedsReload({ activeEndstepTab: async () => 7, hasHook: async () => Promise.reject(new Error('no access')) }),
    ).toBe(false)
  })

  it('is_false_without_waiting_long_when_the_page_never_answers', async () => {
    const started = Date.now()
    const needs = await activeTabNeedsReload({ activeEndstepTab: async () => 7, hasHook: () => new Promise(() => {}) }, 20)
    expect(needs).toBe(false)
    expect(Date.now() - started).toBeLessThan(1000)
  })
})
