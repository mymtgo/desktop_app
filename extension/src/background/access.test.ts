import { describe, expect, it } from 'vitest'
import { ENDSTEP_ORIGINS, hasHostAccess, watchGrants } from './access'

describe('hasHostAccess', () => {
  it('asks_about_the_endstep_host', async () => {
    const asked: unknown[] = []
    const answer = await hasHostAccess({
      contains: async (query) => {
        asked.push(query)
        return false
      },
    })
    expect(answer).toBe(false)
    expect(asked).toEqual([{ origins: ENDSTEP_ORIGINS }])
  })

  it('is_true_when_the_permission_is_held', async () => {
    expect(await hasHostAccess({ contains: async () => true })).toBe(true)
  })

  it('is_true_when_the_browser_cannot_answer', async () => {
    expect(await hasHostAccess({ contains: async () => Promise.reject(new Error('unsupported')) })).toBe(true)
  })
})

describe('watchGrants', () => {
  function setup() {
    let listener: (granted: { origins?: string[] }) => void = () => {}
    let reinjected = 0
    watchGrants({ addListener: (fn) => void (listener = fn) }, async () => void (reinjected += 1))
    return { fire: (origins?: string[]) => listener({ origins }), count: () => reinjected }
  }

  it('reinjects_the_relay_when_endstep_access_is_granted', () => {
    const { fire, count } = setup()
    fire(['https://endstep.cc/*'])
    expect(count()).toBe(1)
  })

  it('ignores_other_grants', () => {
    const { fire, count } = setup()
    fire(['https://example.com/*'])
    fire(undefined)
    expect(count()).toBe(0)
  })
})
