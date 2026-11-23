import { describe, expect, it, vi } from 'vitest'
import type { LastSessionView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: LastSessionCard } = await import('./LastSessionCard.vue')

const SESSION: LastSessionView = {
  startedAtMs: 0,
  endedAtMs: 0,
  startedLabel: 'Oct 7, 2026',
  matches: [
    { token: 'q-1', outcome: 'win', opponentArchetype: 'Qburn', gamesWon: 2, gamesLost: 0 },
    { token: 'q-2', outcome: 'loss', opponentArchetype: 'Qtron', gamesWon: 1, gamesLost: 2 },
    { token: 'q-3', outcome: 'draw', opponentArchetype: 'Unknown', gamesWon: 1, gamesLost: 1 },
    { token: 'q-4', outcome: 'unknown', opponentArchetype: 'Unknown', gamesWon: 0, gamesLost: 0 },
  ],
  record: '1-1',
  duration: '~1h 50m',
}

describe('the Last session card', () => {
  it('marks an unknown outcome ? and a draw D in a neutral mark, never L', () => {
    const mounted = mountFake(LastSessionCard, { session: SESSION })
    expect(mounted.warnings).toEqual([])
    const marks = findAll(mounted.root, (node) => hasClass(node, 'session-mark'))
    expect(marks.map((node) => textOf(node).trim())).toEqual(['W', 'L', 'D', '?'])
    expect(hasClass(marks[3], 'text-muted-foreground')).toBe(true)
    expect(hasClass(marks[3], 'text-destructive')).toBe(false)
  })

  it('heads the card with the date and duration and shows the record large', () => {
    const text = textOf(mountFake(LastSessionCard, { session: SESSION }).root)
    expect(text).toContain('Oct 7, 2026 · ~1h 50m')
    expect(text).toContain('1-1')
  })
})
