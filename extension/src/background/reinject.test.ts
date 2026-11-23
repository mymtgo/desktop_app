import { describe, expect, it } from 'vitest'
import { reinjectRelay, type ReinjectApi } from './reinject'

function fakeApi(tabIds: number[], failing: number[] = []) {
  const calls: Array<[number, string]> = []
  const queries: string[] = []
  const api: ReinjectApi = {
    queryTabs: async (pattern) => {
      queries.push(pattern)
      return tabIds
    },
    runFile: async (tabId, file) => {
      calls.push([tabId, file])
      if (failing.includes(tabId)) throw new Error('No tab with id')
    },
  }
  return { api, calls, queries }
}

describe('reinjectRelay', () => {
  it('the_relay_is_injected_again_into_open_endstep_tabs', async () => {
    const { api, calls, queries } = fakeApi([3, 9])
    expect(await reinjectRelay(api)).toBe(2)
    expect(queries).toEqual(['https://endstep.cc/*'])
    expect(calls).toEqual([
      [3, 'relay.js'],
      [9, 'relay.js'],
    ])
  })

  it('a_tab_that_fails_is_skipped', async () => {
    const { api, calls } = fakeApi([3, 9], [3])
    expect(await reinjectRelay(api)).toBe(1)
    expect(calls).toHaveLength(2)
  })

  it('no_open_tabs_does_nothing', async () => {
    const { api, calls } = fakeApi([])
    expect(await reinjectRelay(api)).toBe(0)
    expect(calls).toEqual([])
  })

  it('a_failing_tab_query_returns_zero', async () => {
    const api: ReinjectApi = {
      queryTabs: async () => {
        throw new Error('boom')
      },
      runFile: async () => {},
    }
    expect(await reinjectRelay(api)).toBe(0)
  })
})
