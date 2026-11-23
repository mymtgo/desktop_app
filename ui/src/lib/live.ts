import type { MatchPage, MatchSummary, PlatformView } from '@/bindings'

/** One pushed event about a match. */
export type Change =
  | { kind: 'updated'; summary: MatchSummary }
  | { kind: 'previewed'; summary: MatchSummary }
  | { kind: 'removed'; token: string }
  | { kind: 'attention'; token: string; reason: string }

/** Everything that happened to one match within one frame. */
export interface MatchChange {
  token: string
  row?: { kind: 'updated'; summary: MatchSummary } | { kind: 'removed' }
  /** A summary to preview, `null` to end the preview, absent for no change. */
  preview?: MatchSummary | null
  attention?: string
}

function tokenOf(change: Change): string {
  return change.kind === 'updated' || change.kind === 'previewed' ? change.summary.token : change.token
}

/** Fold one change into the frame's pending changes (last one wins per match). */
export function collect(pending: Map<string, MatchChange>, change: Change): void {
  const token = tokenOf(change)
  const entry: MatchChange = pending.get(token) ?? { token }
  switch (change.kind) {
    case 'updated':
      entry.row = { kind: 'updated', summary: change.summary }
      // Any later update supersedes a preview, whatever its revision (issue #50).
      entry.preview = null
      break
    case 'removed':
      entry.row = { kind: 'removed' }
      entry.preview = null
      break
    case 'previewed':
      entry.preview = change.summary
      break
    case 'attention':
      entry.attention = change.reason
      // A failed match keeps its row, marked: a preview would hide the mark (issue #50).
      entry.preview = null
      break
  }
  pending.set(token, entry)
}

/**
 * Apply one match's changes to a cached page. A row is replaced only by a
 * summary at least as new (spec 6). `missing` means an update arrived for a
 * match this page does not hold.
 */
export function patchPage(
  page: MatchPage,
  change: MatchChange,
): { page: MatchPage; missing: boolean; removed: boolean; outcomeChanged: boolean } {
  let rows = page.rows
  let total = page.total
  let missing = false
  let removed = false
  let outcomeChanged = false
  const index = rows.findIndex((r) => r.token === change.token)

  if (change.row?.kind === 'updated') {
    const summary = change.row.summary
    if (index === -1) {
      // Only the first page gains rows, and only matches that sort onto it:
      // a rebuild re-deriving old matches must not refetch it every frame.
      const last = rows[rows.length - 1]
      const short = rows.length < page.perPage
      missing = page.page === 1 && (short || last === undefined || summary.startedAtMs >= last.startedAtMs)
    } else if (rows[index].revision <= summary.revision) {
      outcomeChanged = rows[index].outcome !== summary.outcome
      // A push carries no note, rank or rating: keep the row's.
      rows = rows.map((r, i) => (i === index ? { ...summary, id: r.id, notes: r.notes, opponentArchetype: r.opponentArchetype, rank: r.rank, rating: r.rating } : r))
    }
  } else if (change.row?.kind === 'removed' && index !== -1) {
    rows = rows.filter((r) => r.token !== change.token)
    total = Math.max(0, total - 1)
    removed = true
  }

  if (change.attention !== undefined && rows.some((r) => r.token === change.token)) {
    const reason = change.attention
    rows = rows.map((r) => (r.token === change.token ? { ...r, needsAttention: true, attentionReason: reason } : r))
  }

  if (rows === page.rows && total === page.total) {
    return { page, missing, removed, outcomeChanged }
  }
  return { page: { ...page, rows, total }, missing, removed, outcomeChanged }
}

/** What the live client needs from the app (the Query cache and the frame clock). */
export interface LiveDeps {
  /** The platform the pages read; unknown (null or absent) until the context loads, so nothing is ignored. */
  currentPlatform?(): PlatformView | null
  schedule(run: () => void): void
  pages(): Array<{ key: readonly unknown[]; page: MatchPage }>
  setPage(key: readonly unknown[], page: MatchPage): void
  refetchFirstPage(): void
  /** Refetch one cached page (a filtered page whose row stopped matching). */
  refetchPage(key: readonly unknown[]): void
  /** Mark every cached page after the first stale: a removal shifts their rows and total. */
  invalidateLaterPages(): void
  /** Drop every cached matches page and refetch. */
  resetPages(): void
  setPreviews(previews: Map<string, MatchSummary>): void
}

/** Whether a cached page's key (`['matches', page, query]`) carries a result filter. */
function hasResultFilter(key: readonly unknown[]): boolean {
  const query = key[2]
  return typeof query === 'object' && query !== null && (query as { result?: unknown }).result != null
}

/** Collects pushed changes and applies them once per animation frame (spec 6). */
export class LiveClient {
  private pending = new Map<string, MatchChange>()
  private scheduled = false
  private previews = new Map<string, MatchSummary>()
  /** The newest pushed summary per match: a fetched page may be older (spec 6). */
  private latest = new Map<string, MatchSummary>()
  /** Matches a push removed: a page fetched before the removal still lists them. */
  private removed = new Set<string>()

  constructor(private deps: LiveDeps) {}

  push(change: Change): void {
    if (this.isOtherPlatform(change)) {
      return
    }
    if (change.kind === 'updated') {
      const held = this.latest.get(change.summary.token)
      if (held === undefined || held.revision <= change.summary.revision) {
        this.latest.set(change.summary.token, change.summary)
      }
      this.removed.delete(change.summary.token)
    } else if (change.kind === 'removed') {
      this.latest.delete(change.token)
      this.removed.add(change.token)
    }
    collect(this.pending, change)
    if (!this.scheduled) {
      this.scheduled = true
      this.deps.schedule(() => this.flush())
    }
  }

  /**
   * A row or preview of the other platform than the pages read: the lists
   * hold none of them, so it would only mark a page missing and refetch it.
   */
  private isOtherPlatform(change: Change): boolean {
    if (change.kind !== 'updated' && change.kind !== 'previewed') {
      return false
    }
    const current = this.deps.currentPlatform?.() ?? null
    return current !== null && change.summary.platform !== current
  }

  /**
   * Lay pushed changes over a freshly fetched page: a row older than its last
   * push is replaced, a removed match is dropped. Rows storage has caught up
   * with stop being tracked.
   */
  reconcile(page: MatchPage): MatchPage {
    const rows: MatchSummary[] = []
    let total = page.total
    for (const row of page.rows) {
      if (this.removed.has(row.token)) {
        total = Math.max(0, total - 1)
        continue
      }
      const pushed = this.latest.get(row.token)
      if (pushed !== undefined && pushed.revision > row.revision) {
        rows.push({ ...pushed, id: row.id, notes: row.notes, opponentArchetype: row.opponentArchetype, rank: row.rank, rating: row.rating })
      } else {
        rows.push(row)
        this.latest.delete(row.token)
      }
    }
    return { ...page, rows, total }
  }

  /** The tracker restarted: revisions may repeat, so trust a fresh fetch. */
  restart(): void {
    this.pending.clear()
    this.latest.clear()
    this.removed.clear()
    this.previews = new Map()
    this.deps.setPreviews(this.previews)
    this.deps.resetPages()
  }

  flush(): void {
    this.scheduled = false
    const changes = [...this.pending.values()]
    this.pending.clear()
    if (changes.length === 0) {
      return
    }

    let refetch = false
    let removed = false
    const stale: Array<readonly unknown[]> = []
    for (const { key, page } of this.deps.pages()) {
      let next = page
      let outcomeChanged = false
      for (const change of changes) {
        const result = patchPage(next, change)
        next = result.page
        if (result.missing) {
          refetch = true
        }
        removed = removed || result.removed
        outcomeChanged = outcomeChanged || result.outcomeChanged
      }
      if (next !== page) {
        this.deps.setPage(key, next)
      }
      // A result filter may no longer match a row whose outcome changed.
      if (outcomeChanged && hasResultFilter(key)) {
        stale.push(key)
      }
    }
    for (const key of stale) {
      this.deps.refetchPage(key)
    }

    let previewsChanged = false
    const previews = new Map(this.previews)
    for (const change of changes) {
      if (change.preview === null) {
        previewsChanged = previews.delete(change.token) || previewsChanged
      } else if (change.preview !== undefined) {
        previews.set(change.token, change.preview)
        previewsChanged = true
      }
    }
    if (previewsChanged) {
      this.previews = previews
      this.deps.setPreviews(previews)
    }

    if (removed) {
      this.deps.invalidateLaterPages()
    }
    if (refetch) {
      this.deps.refetchFirstPage()
    }
  }
}
