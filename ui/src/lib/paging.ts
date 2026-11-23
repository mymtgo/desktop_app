/** The page to move to when the shown page is empty and beyond the last, else `null`. */
export function pageToShow(shown: { page: number; rows: number; total: number; perPage: number }): number | null {
  if (shown.rows > 0 || shown.page <= 1) {
    return null
  }
  const last = Math.max(1, Math.ceil(shown.total / shown.perPage))
  return Math.min(shown.page - 1, last)
}

/** "Showing 1–50 of 120 matches" (0.x deck matches), empty when there are none. */
export function showingText(shown: { page: number; perPage: number; total: number }): string {
  if (shown.total === 0) return ''
  const first = (shown.page - 1) * shown.perPage + 1
  const last = Math.min(shown.page * shown.perPage, shown.total)
  return `Showing ${first}–${last} of ${shown.total} matches`
}
