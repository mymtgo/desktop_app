/**
 * Run a fetch, but keep the cached value if a push arrived while it ran: the
 * push is newer than anything the fetch read (a status has no revision).
 */
export async function fetchUnlessPushed<T>(
  pushes: () => number,
  fetch: () => Promise<T>,
  cached: () => T | undefined,
): Promise<T> {
  const before = pushes()
  const fetched = await fetch()
  if (pushes() !== before) {
    const held = cached()
    if (held !== undefined) {
      return held
    }
  }
  return fetched
}

/** Counts one kind of push, for `fetchUnlessPushed`. */
export function pushCounter(): { count: () => number; note: () => void } {
  let pushes = 0
  return {
    count: () => pushes,
    note: () => {
      pushes += 1
    },
  }
}
