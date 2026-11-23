import { archetypeFormatKey } from './archetypePicker'

/**
 * Deck ids travel from a deck card to a sidebar archetype row through the
 * native DataTransfer under a private type (0.x `lib/deckDrag`). Drop
 * targets only see `types` during dragover (the data is protected until
 * the drop), so the type tells a deck drag from a stray text or file drag.
 */
export const DECK_DRAG_TYPE = 'application/x-mymtgo-deck-ids'

export function writeDeckDrag(dataTransfer: DataTransfer, ids: number[]): void {
  dataTransfer.setData(DECK_DRAG_TYPE, JSON.stringify(ids))
  dataTransfer.effectAllowed = 'move'
}

export function hasDeckDrag(dataTransfer: DataTransfer | null): boolean {
  return !!dataTransfer && Array.from(dataTransfer.types).includes(DECK_DRAG_TYPE)
}

export function readDeckDrag(dataTransfer: DataTransfer | null): number[] {
  if (!dataTransfer) return []
  try {
    const parsed: unknown = JSON.parse(dataTransfer.getData(DECK_DRAG_TYPE))
    return Array.isArray(parsed) ? parsed.filter((id): id is number => typeof id === 'number') : []
  } catch {
    return []
  }
}

/** A selected card drags the whole selection; any other card itself. */
export function dragIds(deckId: number, selected: boolean, selectedIds: number[]): number[] {
  return selected ? selectedIds : [deckId]
}

/** The drag ghost's text: the deck's name, or how many decks. */
export function deckDragLabel(count: number, singleName: string): string {
  return count > 1 ? `${count} decks` : singleName
}

/**
 * Whether a row takes the decks being dragged (0.x `acceptsDrag`): an
 * archetype row only its own format; Unclassified and "Other archetype…"
 * (no format) and a drag of unknown format anything.
 */
export function acceptsDrag(dragFormat: string | null, rowFormat: string | null): boolean {
  if (dragFormat === null || rowFormat === null) return true
  return archetypeFormatKey(dragFormat) === rowFormat
}

/**
 * Replace the browser's drag ghost (a copy of the whole card) with a small
 * pill naming what is dragged. The pill has to be in the DOM when
 * `setDragImage` runs, so it is added off screen and removed right after.
 */
export function setDeckDragImage(dataTransfer: DataTransfer, label: string): void {
  const pill = document.createElement('div')
  pill.textContent = label
  pill.style.cssText = [
    'position:fixed',
    'top:-1000px',
    'left:-1000px',
    'max-width:240px',
    'padding:4px 10px',
    'border-radius:6px',
    'border:1px solid rgba(255,255,255,0.15)',
    'background:#171717',
    'color:#fff',
    'font-size:12px',
    'font-weight:500',
    'line-height:1.4',
    'white-space:nowrap',
    'overflow:hidden',
    'text-overflow:ellipsis',
    'box-shadow:0 4px 12px rgba(0,0,0,0.5)',
    'pointer-events:none',
  ].join(';')
  document.body.appendChild(pill)
  dataTransfer.setDragImage(pill, 12, 14)
  setTimeout(() => pill.remove(), 0)
}
