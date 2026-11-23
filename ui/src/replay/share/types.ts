import type { ReplayShareState } from '@/bindings'

export type { ReplayShareState }

/** What the dialog body needs to draw itself (the state is null until it loads). */
export type ReplayShareView = {
  share: ReplayShareState | null
  busy: boolean
  error: string | null
  copied: boolean
}

/** How long "Copied" stays on the copy button. */
export const COPIED_MS = 2000
