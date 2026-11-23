import { describe, expect, it } from 'vitest'
import type { MatchPage, MatchSummary, PlatformView } from '@/bindings'
import { LiveClient, type LiveDeps } from './live'

function row(token: string, revision: number, overrides: Partial<MatchSummary> = {}): MatchSummary {
  return {
    id: null,
    token,
    opponent: 'Qopponent',
    format: 'Modern',
    matchType: 'Constructed',
    platform: 'mtgo',
    bestOf: null,
    state: 'in_progress',
    outcome: 'unknown',
    tentativeOutcome: null,
    wins: 0,
    losses: 0,
    needsAttention: false,
    attentionReason: null,
    startedAtMs: 1_000,
    endedAtMs: null,
    revision,
    notes: null,
    gameResults: [],
    manual: false,
    limited: false,
    opponentArchetype: null,
    ...overrides,
  }
}

function page(rows: MatchSummary[], number = 1): MatchPage {
  return { rows, total: rows.length, page: number, perPage: 50, unknownArchetypeCount: 0, archetypes: [] }
}

/** Fake app wiring: one cached first page, frames run on demand. */
function harness(initial: MatchPage | null, platform: PlatformView | null = null) {
  const frames: Array<() => void> = []
  const cache = new Map<string, MatchPage>()
  if (initial) cache.set(JSON.stringify(['matches', initial.page]), initial)
  const calls = { setPage: 0, refetch: 0, reset: 0, invalidateLater: 0 }
  const refetchedPages: Array<readonly unknown[]> = []
  let previews = new Map<string, MatchSummary>()
  const deps: LiveDeps = {
    currentPlatform: () => platform,
    schedule: (run) => frames.push(run),
    pages: () => [...cache.entries()].map(([key, value]) => ({ key: JSON.parse(key), page: value })),
    setPage: (key, value) => {
      calls.setPage += 1
      cache.set(JSON.stringify(key), value)
    },
    refetchFirstPage: () => {
      calls.refetch += 1
    },
    invalidateLaterPages: () => {
      calls.invalidateLater += 1
    },
    refetchPage: (key) => {
      refetchedPages.push(key)
    },
    resetPages: () => {
      calls.reset += 1
      cache.clear()
    },
    setPreviews: (value) => {
      previews = value
    },
  }
  const client = new LiveClient(deps)
  return {
    client,
    calls,
    refetchedPages,
    frames,
    runFrame: () => frames.splice(0).forEach((run) => run()),
    rows: () => cache.get(JSON.stringify(['matches', 1]))?.rows ?? [],
    cache,
    previews: () => previews,
  }
}

describe('LiveClient', () => {
  it('ignores a push of the other platform: no refetch, no preview, no row', () => {
    const h = harness(page([row('t1', 1)]), 'mtgo')
    h.client.push({ kind: 'updated', summary: row('arena-1', 1, { platform: 'arena' }) })
    h.client.push({ kind: 'previewed', summary: row('arena-2', 1, { platform: 'arena' }) })
    h.runFrame()
    expect(h.calls.refetch).toBe(0)
    expect(h.calls.setPage).toBe(0)
    expect(h.previews().size).toBe(0)
    expect(h.rows().map((r) => r.token)).toEqual(['t1'])
    h.client.push({ kind: 'updated', summary: row('mtgo-2', 1) })
    h.runFrame()
    expect(h.calls.refetch).toBe(1)
  })

  it('holds every platform while the context is unknown', () => {
    const h = harness(page([row('t1', 1)]), null)
    h.client.push({ kind: 'updated', summary: row('arena-1', 1, { platform: 'arena' }) })
    h.runFrame()
    expect(h.calls.refetch).toBe(1)
  })

  it('refetches a result-filtered page when a held row changes outcome, not an unfiltered one', () => {
    const h = harness(page([row('t1', 1)]))
    h.cache.set(JSON.stringify(['matches', 1, { result: 'win', page: 1 }]), page([row('t1', 1)]))
    h.cache.set(JSON.stringify(['matches', 1, { result: null, page: 1 }]), page([row('t1', 1)]))
    h.client.push({ kind: 'updated', summary: row('t1', 2, { outcome: 'loss', state: 'complete' }) })
    h.runFrame()
    expect(h.refetchedPages).toEqual([['matches', 1, { result: 'win', page: 1 }]])
  })

  it('does not refetch a filtered page when the outcome stays the same', () => {
    const h = harness(page([row('t1', 1)]))
    h.cache.set(JSON.stringify(['matches', 1, { result: 'win', page: 1 }]), page([row('t1', 1)]))
    h.client.push({ kind: 'updated', summary: row('t1', 2, { wins: 1 }) })
    h.runFrame()
    expect(h.refetchedPages).toEqual([])
  })

  it('a matched removal invalidates the later pages', () => {
    const h = harness(page([row('t1', 1), row('t2', 1)]))
    h.client.push({ kind: 'removed', token: 't1' })
    h.runFrame()
    expect(h.calls.invalidateLater).toBe(1)
  })

  it('a removal of an unknown match leaves the later pages alone', () => {
    const h = harness(page([row('t1', 1)]))
    h.client.push({ kind: 'removed', token: 'other' })
    h.runFrame()
    expect(h.calls.invalidateLater).toBe(0)
  })

  it('an update patches its row in place', () => {
    const h = harness(page([row('t1', 1), row('t2', 1)]))
    h.client.push({ kind: 'updated', summary: row('t1', 2, { wins: 1 }) })
    expect(h.rows()[0].wins).toBe(0) // nothing until the frame
    h.runFrame()
    expect(h.rows().map((r) => [r.token, r.revision, r.wins])).toEqual([
      ['t1', 2, 1],
      ['t2', 1, 0],
    ])
  })

  it('an update keeps the row note and id, which pushes never carry', () => {
    const h = harness(page([row('t1', 1, { id: 7, notes: 'Q note' })]))
    h.client.push({ kind: 'updated', summary: row('t1', 2, { wins: 1 }) })
    h.runFrame()
    expect(h.rows()[0]).toMatchObject({ id: 7, revision: 2, wins: 1, notes: 'Q note' })
    // A page fetched before the push keeps its stored note and id too.
    const late = h.client.reconcile(page([row('t1', 1, { id: 7, notes: 'Q note' })]))
    expect(late.rows[0]).toMatchObject({ id: 7, revision: 2, notes: 'Q note' })
  })

  it('an update keeps the stored rank, which pushes carry as null', () => {
    const rank = { class: 'Gold', level: 2, step: 3, mythicPercentile: null, mythicPlacement: null, label: 'Gold 2' }
    const h = harness(page([row('t1', 1, { id: 7, rank })]))
    h.client.push({ kind: 'updated', summary: row('t1', 2, { rank: null }) })
    h.runFrame()
    expect(h.rows()[0].rank).toEqual(rank)
    const late = h.client.reconcile(page([row('t1', 1, { id: 7, rank })]))
    expect(late.rows[0].rank).toEqual(rank)
  })

  it('an update keeps the stored rating, which pushes carry as null', () => {
    const rating = { before: 1371, after: 1336, delta: -35 }
    const h = harness(page([row('t1', 1, { id: 7, rating })]))
    h.client.push({ kind: 'updated', summary: row('t1', 2, {}) })
    h.runFrame()
    expect(h.rows()[0].rating).toEqual(rating)
    const late = h.client.reconcile(page([row('t1', 1, { id: 7, rating })]))
    expect(late.rows[0].rating).toEqual(rating)
  })

  it('an older revision never replaces a newer row', () => {
    const h = harness(page([row('t1', 5, { wins: 2 })]))
    h.client.push({ kind: 'updated', summary: row('t1', 4, { wins: 1 }) })
    h.runFrame()
    expect(h.rows()[0]).toMatchObject({ revision: 5, wins: 2 })
    expect(h.calls.setPage).toBe(0)
  })

  it('a burst of updates patches each match once per frame', () => {
    const tokens = Array.from({ length: 10 }, (_, i) => `t${i}`)
    const h = harness(page(tokens.map((token) => row(token, 0))))
    for (let revision = 1; revision <= 100; revision += 1) {
      for (const token of tokens) {
        h.client.push({ kind: 'updated', summary: row(token, revision) })
      }
    }
    expect(h.frames.length).toBe(1)
    h.runFrame()
    expect(h.calls.setPage).toBe(1)
    expect(h.rows().every((r) => r.revision === 100)).toBe(true)
  })

  it('an update for a match the first page does not hold refetches it', () => {
    const h = harness(page([row('t1', 1)]))
    h.client.push({ kind: 'updated', summary: row('new', 1, { startedAtMs: 9_000 }) })
    h.runFrame()
    expect(h.calls.refetch).toBe(1)
  })

  it('a removal drops the row and its preview', () => {
    const h = harness(page([row('t1', 1), row('t2', 1)]))
    h.client.push({ kind: 'previewed', summary: row('t1', 0, { wins: 1 }) })
    h.runFrame()
    expect(h.previews().has('t1')).toBe(true)
    h.client.push({ kind: 'removed', token: 't1' })
    h.runFrame()
    expect(h.rows().map((r) => r.token)).toEqual(['t2'])
    expect(h.cache.get(JSON.stringify(['matches', 1]))?.total).toBe(1)
    expect(h.previews().has('t1')).toBe(false)
  })

  it('a preview shows until any later update supersedes it, whatever its revision', () => {
    const h = harness(page([row('t1', 7)]))
    h.client.push({ kind: 'previewed', summary: row('t1', 0, { wins: 1 }) })
    h.runFrame()
    expect(h.previews().get('t1')?.wins).toBe(1)
    // An update older than the cached row still ends the preview (issue #50).
    h.client.push({ kind: 'updated', summary: row('t1', 6) })
    h.runFrame()
    expect(h.previews().has('t1')).toBe(false)
  })

  it('a later update in the same frame supersedes a preview (issue #50)', () => {
    const h = harness(page([row('t1', 7)]))
    // The tracker previewed, then changed the match later in the same tick.
    h.client.push({ kind: 'previewed', summary: row('t1', 0, { wins: 1 }) })
    h.client.push({ kind: 'updated', summary: row('t1', 6) })
    h.runFrame()
    expect(h.previews().has('t1')).toBe(false)
    // A preview after the update in one frame still shows.
    h.client.push({ kind: 'updated', summary: row('t1', 8) })
    h.client.push({ kind: 'previewed', summary: row('t1', 0, { wins: 2 }) })
    h.runFrame()
    expect(h.previews().get('t1')?.wins).toBe(2)
  })

  it('attention ends a preview, so the row shows it (issue #50)', () => {
    const h = harness(page([row('t1', 3)]))
    h.client.push({ kind: 'previewed', summary: row('t1', 0, { wins: 1 }) })
    h.runFrame()
    h.client.push({ kind: 'attention', token: 't1', reason: 'segments and game ids disagree' })
    h.runFrame()
    expect(h.previews().has('t1')).toBe(false)
    expect(h.rows()[0].needsAttention).toBe(true)
  })

  it('attention marks the row', () => {
    const h = harness(page([row('t1', 3)]))
    h.client.push({ kind: 'attention', token: 't1', reason: 'segments and game ids disagree' })
    h.runFrame()
    expect(h.rows()[0]).toMatchObject({ needsAttention: true, attentionReason: 'segments and game ids disagree' })
  })

  it('an update for an old match never refetches a full first page', () => {
    const full: MatchPage = { rows: [row('a', 1, { startedAtMs: 5_000 }), row('b', 1, { startedAtMs: 4_000 })], total: 9, page: 1, perPage: 2, unknownArchetypeCount: 0, archetypes: [] }
    const h = harness(full)
    // A rebuild re-derives old matches: they can never join this page.
    h.client.push({ kind: 'updated', summary: row('old', 3, { startedAtMs: 10 }) })
    h.runFrame()
    expect(h.calls.refetch).toBe(0)
    h.client.push({ kind: 'updated', summary: row('new', 1, { startedAtMs: 6_000 }) })
    h.runFrame()
    expect(h.calls.refetch).toBe(1)
  })

  it('a page fetched before a push never overwrites it', () => {
    const h = harness(page([row('t1', 4)]))
    h.client.push({ kind: 'updated', summary: row('t1', 5, { wins: 1 }) })
    h.runFrame()
    // The list command ran before the bridge saw revision 5.
    const late = h.client.reconcile(page([row('t1', 4), row('t2', 1)]))
    expect(late.rows.map((r) => [r.token, r.revision])).toEqual([
      ['t1', 5],
      ['t2', 1],
    ])
    // Storage caught up: its row is used again.
    expect(h.client.reconcile(page([row('t1', 6)])).rows[0].revision).toBe(6)
  })

  it('a removed match stays hidden in a page fetched before the removal', () => {
    const h = harness(page([row('t1', 2), row('t2', 1)]))
    h.client.push({ kind: 'removed', token: 't1' })
    const late = h.client.reconcile(page([row('t1', 2), row('t2', 1)]))
    expect(late.rows.map((r) => r.token)).toEqual(['t2'])
    expect(late.total).toBe(1)
    // A later update brings it back (a match can revive).
    h.client.push({ kind: 'updated', summary: row('t1', 3) })
    expect(h.client.reconcile(page([row('t1', 2)])).rows[0].revision).toBe(3)
  })

  it('a restart resets revision floors and refetches', () => {
    const h = harness(page([row('t1', 9)]))
    h.client.push({ kind: 'previewed', summary: row('t1', 0) })
    h.runFrame()
    h.client.push({ kind: 'updated', summary: row('t1', 10) })
    h.client.restart()
    expect(h.calls.reset).toBe(1)
    expect(h.previews().size).toBe(0)
    // The refetched page carries the restarted tracker's lower revision.
    h.cache.set(JSON.stringify(['matches', 1]), page([row('t1', 4)]))
    h.runFrame() // the update queued before the restart was dropped
    expect(h.rows()[0].revision).toBe(4)
    h.client.push({ kind: 'updated', summary: row('t1', 5, { wins: 1 }) })
    h.runFrame()
    expect(h.rows()[0]).toMatchObject({ revision: 5, wins: 1 })
    // Pushed revisions from before the restart are forgotten too.
    h.client.restart()
    expect(h.client.reconcile(page([row('t1', 2)])).rows[0].revision).toBe(2)
  })
})
