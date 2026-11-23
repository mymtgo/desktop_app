import { describe, expect, it, vi } from 'vitest'
import type { DeletedLeagueView, DeletedMatchView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('@/composables/useMatchEdit', () => ({ useMatchEdit: () => ({ restore: async () => true }) }))
vi.mock('@/composables/useLeagueManage', () => ({ useLeagueManage: () => ({ restoreLeague: async () => true }) }))

const { default: ArchivedMatchesTable } = await import('./ArchivedMatchesTable.vue')
const { default: ArchivedLeaguesTable } = await import('./ArchivedLeaguesTable.vue')

function match(token: string, deckName: string | null, leagueName: string | null = null, leagueRemoved = false): DeletedMatchView {
  return {
    token,
    startedAtMs: 1_800_000_000_000,
    hiddenAtMs: 1_800_000_100_000,
    opponent: 'Qopp',
    result: 'win',
    deckName,
    leagueName,
    leagueRemoved,
    deletedWithLeague: null,
  }
}

function league(runKey: string, deckName: string | null, format: string | null): DeletedLeagueView {
  return { runKey, name: 'Q league', startedAtMs: 1_800_000_000_000, removedAtMs: 1_800_000_100_000, wins: 3, losses: 1, deckName, format }
}

function deckCells(root: Parameters<typeof findAll>[0]) {
  return findAll(root, (node) => node.props['data-cell'] === 'deck')
}

describe('the Deck column of the Archives tables', () => {
  it('shows a match deck, and a muted dash without one', () => {
    const mounted = mountFake(ArchivedMatchesTable, { matches: [match('q-1', 'Qburn'), match('q-2', null)] })
    expect(mounted.warnings).toEqual([])
    const [withDeck, without] = deckCells(mounted.root)
    expect(textOf(withDeck).trim()).toBe('Qburn')
    expect(hasClass(withDeck, 'text-muted-foreground')).toBe(false)
    expect(textOf(without).trim()).toBe('-')
    expect(hasClass(without, 'text-muted-foreground')).toBe(true)
  })

  it('shows a league deck, the format muted without one, else a muted dash', () => {
    const mounted = mountFake(ArchivedLeaguesTable, {
      leagues: [league('manual:1', 'Qburn', 'Modern'), league('manual:2', null, 'Pauper'), league('manual:3', null, null)],
    })
    expect(mounted.warnings).toEqual([])
    const [withDeck, formatOnly, neither] = deckCells(mounted.root)
    expect(textOf(withDeck).trim()).toBe('Qburn')
    expect(hasClass(withDeck, 'text-muted-foreground')).toBe(false)
    expect(textOf(formatOnly).trim()).toBe('Pauper')
    expect(hasClass(formatOnly, 'text-muted-foreground')).toBe(true)
    expect(textOf(neither).trim()).toBe('-')
  })

  it('shows the match league, marks a deleted one, and a muted dash for none', () => {
    const mounted = mountFake(ArchivedMatchesTable, {
      matches: [match('q-1', null, 'Q league'), match('q-2', null, 'Q gone', true), match('q-3', null)],
    })
    expect(mounted.warnings).toEqual([])
    const [live, gone, none] = findAll(mounted.root, (node) => node.props['data-cell'] === 'league')
    expect(textOf(live).trim()).toBe('Q league')
    expect(textOf(gone)).toContain('Q gone')
    expect(textOf(gone)).toContain('(deleted)')
    expect(textOf(none).trim()).toBe('-')
  })
})
