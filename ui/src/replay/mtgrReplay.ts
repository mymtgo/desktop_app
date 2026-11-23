import { parse, type Game } from '@mtgreplay/mtgr'
import type { MtgrReplayCard, MtgrReplayPrinting } from '@/bindings'
import { cardImageSrc } from '@/lib/cards'
import type { CardImageResolver } from '@/replay-mtgr/types'

/**
 * mymtgo.com's card image redirect, `GET {base}/arena/{grpId}` (the API's
 * `card-images.show`). Fixed rather than the configured API base: the
 * window cannot read `MYMTGO_API_URL`, and its CSP must name the host.
 */
export const CARD_IMAGES_URL = 'https://mymtgo.com/api/card-images'

/** The games of an Arena replay's `.mtgr` text, parsed after NFC (spec 4.4). */
export function mtgrGames(text: string): Game[] {
  return parse(text.normalize('NFC')).file.games
}

/** An Arena source id's grpId (`arena:75000`), or null. */
function arenaId(sources: readonly string[] | undefined): string | null {
  const source = sources?.find((item) => /^arena:\d+$/.test(item))
  return source ? source.slice('arena:'.length) : null
}

/**
 * The viewer's `card-image` for an Arena replay. A front face takes, in order:
 * the downloaded image of the exact printing played (its Arena source id, through
 * the `card` scheme), that printing's stored remote image, the downloaded image of
 * the row chosen for the card's name, the printing played through mymtgo.com, the
 * name's stored remote image, else none. The printing comes first because printings
 * of one name differ (a basic land has dozens, each with its own art), where the
 * name's row is one of them. The back face of a double-faced card has its own
 * picture, asked for by the card's Arena id (`/back`), and none without one. Every
 * half of a split card, Room or Adventure is the card's one picture, so the viewer
 * shows it once. A card named `?` (null) has no entry: only its Arena id is tried,
 * and the viewer shows the card back if that fails. A token takes its entry by the
 * name the record gives it ("Map Token"), filled from a token row. A recorded MTGO
 * game has no `arena:` source: its cards take the name's downloaded image, then its
 * stored remote image, never a mymtgo.com `/arena/` URL.
 */
export function mtgrCardImage(
  cards: readonly MtgrReplayCard[],
  printings: readonly MtgrReplayPrinting[] = [],
): CardImageResolver {
  const byName = new Map(cards.map((card) => [card.name, card]))
  const byArenaId = new Map(printings.map((printing) => [String(printing.grpId), printing]))
  return (name, face, sources) => {
    const card = name === null ? undefined : byName.get(name)
    const id = arenaId(sources)
    if (face !== undefined && card?.doubleFaced) {
      if (id !== null) return `${CARD_IMAGES_URL}/arena/${id}/back`
      // An Endstep card has no Arena id: its stored Scryfall front picture names the back one.
      return card.image?.includes('/front/') ? card.image.replace('/front/', '/back/') : null
    }
    const printing = id === null ? undefined : byArenaId.get(id)
    const played = printing ? cardImageSrc(printing.image, printing.localImage) : null
    if (played) return played
    if (card?.localImage) return cardImageSrc(null, card.localImage)
    if (id !== null) return `${CARD_IMAGES_URL}/arena/${id}`
    return card?.image ?? null
  }
}
