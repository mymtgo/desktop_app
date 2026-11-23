import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { MtgrReplayCard, MtgrReplayPrinting, MtgrReplayView } from '@/bindings'
import lossRecord from '../../../crates/arena-game/src/snapshots/arena_game__mtgr__tests__loss_record.snap?raw'
import endstepRecord from '../../../crates/endstep-match/tests/snapshots/mtgr__ranked_bo3_record.snap?raw'
import { findAll, mountFake, textOf } from './replayTestHarness'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string, scheme: string) => `http://${scheme}.localhost/${encodeURIComponent(path)}`,
}))

const { default: MtgrReplay } = await import('./MtgrReplay.vue')
const { CARD_IMAGES_URL, mtgrCardImage, mtgrGames } = await import('./mtgrReplay')

/** Task 5's loss golden (three games): the `.mtgr` text after the insta header. */
function goldenText(snapshot: string = lossRecord): string {
  const lines = snapshot.split('\n')
  let seen = 0
  for (const [i, line] of lines.entries()) {
    if (line === '---' && ++seen === 2) return lines.slice(i + 1).join('\n')
  }
  throw new Error('snapshot has no insta header')
}

function arenaView(extra: Partial<MtgrReplayView> = {}) {
  return { kind: 'mtgr' as const, text: goldenText(), gameIndex: 1, cards: [], printings: [], ...extra }
}

// The window and document the viewer listens on (keys, fullscreen), with no DOM.
beforeEach(() => {
  const listeners = { addEventListener: () => {}, removeEventListener: () => {} }
  vi.stubGlobal('window', { ...listeners, localStorage: { getItem: () => null, setItem: () => {} } })
  vi.stubGlobal('document', { ...listeners, querySelector: () => null, fullscreenElement: null })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the Endstep replay host', () => {
  it('an endstep record plays', async () => {
    const text = goldenText(endstepRecord)
    const games = mtgrGames(text)
    expect(games).toHaveLength(3)
    const view = { kind: 'mtgr' as const, text, gameIndex: 0, cards: [], printings: [] }
    const mounted = mountFake(MtgrReplay, { view })
    await mounted.settle()
    expect(mounted.warnings).toEqual([])
    const shown = textOf(mounted.root)
    expect(shown).toContain('Opponent')
    // Only game 1's opening hand holds Giant's Boulder, so these lines prove game 1 is shown.
    expect(shown).toContain("Player draws Giant's Boulder by opening hand")
    expect(shown).toContain('Opponent plays Hedge Maze tapped')
    expect(shown).not.toContain('This replay has no games to show.')
    mounted.unmount()
  })
})

describe('the Arena replay host', () => {
  it('mounts_the_mtgr_viewer_for_an_arena_view', async () => {
    // The viewer as a spy first, for the props the host passes.
    let seen: Record<string, unknown> | null = null
    const Spy = {
      props: ['games', 'gameIndex', 'cardImage'],
      setup(props: Record<string, unknown>) {
        seen = props
        return () => null
      },
    }
    vi.doMock('@/replay-mtgr/ReplayViewer.vue', () => ({ default: Spy }))
    vi.resetModules()
    const { default: Host } = await import('./MtgrReplay.vue')
    mountFake(Host, { view: arenaView() })
    vi.doUnmock('@/replay-mtgr/ReplayViewer.vue')
    expect(seen).not.toBeNull()
    const props = seen as unknown as { games: unknown[]; gameIndex: number; cardImage: unknown }
    expect(props.games).toHaveLength(3)
    expect(props.gameIndex).toBe(1)
    expect(typeof props.cardImage).toBe('function')

    // The real viewer renders the requested game with no warnings.
    const mounted = mountFake(MtgrReplay, { view: arenaView() })
    await mounted.settle()
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).toContain('Opponent')
    mounted.unmount()
  })

  it('a_src_parsed_from_the_text_reaches_the_image_resolver', async () => {
    // The real parser and viewer, with the resolver recorded: every `src`
    // the viewer hands it was read from the record's text.
    const calls: { name: string | null; sources: readonly string[] | undefined }[] = []
    vi.doMock('./mtgrReplay', async (importOriginal) => {
      const original = await importOriginal<typeof import('./mtgrReplay')>()
      return {
        ...original,
        mtgrCardImage: (...args: Parameters<typeof original.mtgrCardImage>) => {
          const image = original.mtgrCardImage(...args)
          return (...call: Parameters<typeof image>) => {
            calls.push({ name: call[0], sources: call[2] })
            return image(...call)
          }
        },
      }
    })
    vi.resetModules()
    const { default: Host } = await import('./MtgrReplay.vue')
    vi.doUnmock('./mtgrReplay')
    const text = goldenText()
    const sourced = new Set([...text.matchAll(/ src (arena:\d+)/g)].map((match) => match[1]))
    const mounted = mountFake(Host, { view: arenaView({ text }) })
    await mounted.settle()
    // Play on to the turns, where cards are on the board.
    const next = findAll(mounted.root, (node) => node.tag === 'button' && node.props.title === 'Next turn (Shift+→)')[0]
    for (let turn = 0; turn < 3; turn++) {
      ;(next.props.onClick as () => void)()
      await mounted.settle()
    }
    mounted.unmount()
    const received = calls.flatMap((call) => call.sources ?? [])
    expect(received.length).toBeGreaterThan(0)
    for (const source of received) expect(sourced.has(source)).toBe(true)
  })

  it('normalises_before_parsing', () => {
    // "Opponent" plus an e and a combining acute accent (NFD).
    const decomposed = 'Opponente\u0301'
    const text = goldenText().replaceAll('"Opponent"', `"${decomposed}"`)
    expect(text.normalize('NFC')).not.toBe(text)
    const games = mtgrGames(text)
    expect(games).toHaveLength(3)
    const players = games[0].tags.filter((tag) => /^Player\d$/.test(tag.name)).map((tag) => tag.value)
    expect(players).toContain('Opponent\u00e9')
    expect(players).not.toContain(decomposed)
  })

  it('card_image_reads_the_map', () => {
    const cards: MtgrReplayCard[] = [
      { name: 'Qlocal', image: 'https://cards.example/l.jpg', localImage: 'l.jpg', doubleFaced: false },
      { name: 'Qremote', image: 'https://cards.example/r.jpg', localImage: null, doubleFaced: false },
      { name: 'Qfront // Qback', image: 'https://cards.example/f.jpg', localImage: 'f.jpg', doubleFaced: true },
      { name: 'Qleft // Qright', image: 'https://cards.example/s.jpg', localImage: null, doubleFaced: false },
    ]
    const image = mtgrCardImage(cards)
    // The downloaded file through the `card` scheme first, as MTGO replays show `local_image`.
    expect(image('Qlocal')).toBe('http://card.localhost/l.jpg')
    expect(image('Qlocal', undefined, ['arena:1'])).toBe('http://card.localhost/l.jpg')
    // Then the printing played, by its Arena source id.
    expect(image('Qremote', undefined, ['mtgo:7', 'arena:75000'])).toBe(`${CARD_IMAGES_URL}/arena/75000`)
    expect(CARD_IMAGES_URL).toBe('https://mymtgo.com/api/card-images')
    // A card named `?` (null) is tried by its Arena id only, never by the map.
    expect(image(null, undefined, ['arena:106504'])).toBe(`${CARD_IMAGES_URL}/arena/106504`)
    expect(image(null)).toBeNull()
    // Then the stored remote image.
    expect(image('Qremote')).toBe('https://cards.example/r.jpg')
    // A double-faced card's back face has its own picture: asked for by the card's Arena id.
    expect(image('Qfront // Qback', 'Qback', ['arena:78507'])).toBe(`${CARD_IMAGES_URL}/arena/78507/back`)
    expect(image('Qfront // Qback', undefined, ['arena:78507'])).toBe('http://card.localhost/f.jpg')
    // Without an id there is no back face picture; the front would be the wrong face.
    expect(image('Qfront // Qback', 'Qback')).toBeNull()
    // An Endstep card has no Arena id: the back is the stored Scryfall front picture's back.
    const scryfall: MtgrReplayCard[] = [
      { name: 'Qdfc // Qflip', image: 'https://cards.scryfall.io/large/front/a/b/x.jpg', localImage: null, doubleFaced: true },
    ]
    expect(mtgrCardImage(scryfall)('Qdfc // Qflip', 'Qflip')).toBe('https://cards.scryfall.io/large/back/a/b/x.jpg')
    // A split card, Room or Adventure prints every half on one picture: the same URL as the card.
    expect(image('Qleft // Qright', 'Qright', ['arena:74013'])).toBe(`${CARD_IMAGES_URL}/arena/74013`)
    expect(image('Qleft // Qright', 'Qright')).toBe('https://cards.example/s.jpg')
    // No image at all.
    expect(image('Qunknown')).toBeNull()
  })

  it('an_mtgo_card_without_sources_uses_its_name_image', () => {
    const cards: MtgrReplayCard[] = [
      { name: 'Qlocal', image: 'https://cards.example/l.jpg', localImage: 'l.jpg', doubleFaced: false },
      { name: 'Qremote', image: 'https://cards.example/r.jpg', localImage: null, doubleFaced: false },
    ]
    const image = mtgrCardImage(cards)
    // An MTGO card source (`mtgo:<id>`) is no Arena id: never the mymtgo.com /arena/ URL.
    expect(image('Qlocal', undefined, ['mtgo:7'])).toBe('http://card.localhost/l.jpg')
    expect(image('Qremote', undefined, ['mtgo:7'])).toBe('https://cards.example/r.jpg')
    expect(image('Qremote')).toBe('https://cards.example/r.jpg')
    expect(image('Qunknown', undefined, ['mtgo:9'])).toBeNull()
  })

  it('card_image_prefers_the_printing_played', () => {
    // Two printings of one name: the name's row is the first, the match played the second.
    const cards: MtgrReplayCard[] = [
      { name: 'Qforest', image: 'https://cards.example/forest-a.jpg', localImage: 'a.jpg', doubleFaced: false },
      { name: 'Qdfc // Qback', image: 'https://cards.example/dfc.jpg', localImage: 'dfc.jpg', doubleFaced: true },
    ]
    const printings: MtgrReplayPrinting[] = [
      { grpId: 1, image: 'https://cards.example/forest-a.jpg', localImage: 'a.jpg' },
      { grpId: 2, image: 'https://cards.example/forest-b.jpg', localImage: 'b.jpg' },
      { grpId: 3, image: 'https://cards.example/forest-c.jpg', localImage: null },
      { grpId: 4, image: null, localImage: null },
    ]
    const image = mtgrCardImage(cards, printings)
    // The exact printing's file, not the name's row.
    expect(image('Qforest', undefined, ['arena:2'])).toBe('http://card.localhost/b.jpg')
    // No file for that printing: its own remote image, not the name's file.
    expect(image('Qforest', undefined, ['arena:3'])).toBe('https://cards.example/forest-c.jpg')
    // A printing with no images, or none stored: the name's row, then the old order.
    expect(image('Qforest', undefined, ['arena:4'])).toBe('http://card.localhost/a.jpg')
    expect(image('Qforest', undefined, ['arena:9'])).toBe('http://card.localhost/a.jpg')
    expect(image('Qforest')).toBe('http://card.localhost/a.jpg')
    // A back face is still asked for by its Arena id, whatever the printings say.
    expect(image('Qdfc // Qback', 'Qback', ['arena:2'])).toBe(`${CARD_IMAGES_URL}/arena/2/back`)
    expect(image('Qdfc // Qback', undefined, ['arena:2'])).toBe('http://card.localhost/b.jpg')
  })

  it('missing_card_data_shows_the_message', () => {
    const mounted = mountFake(MtgrReplay, { view: { kind: 'missingCardData' } })
    expect(textOf(mounted.root).trim()).toBe(
      "Some cards in this game aren't in card data yet. The replay will open once they are.",
    )
    expect(mounted.warnings).toEqual([])
  })

  it('no_share_button_on_arena', async () => {
    const mounted = mountFake(MtgrReplay, { view: arenaView() })
    await mounted.settle()
    const labels = findAll(mounted.root, (node) => node.tag === 'button').map((node) => textOf(node).trim())
    expect(labels.length).toBeGreaterThan(0)
    expect(labels.some((label) => /share/i.test(label))).toBe(false)
    expect(textOf(mounted.root)).not.toMatch(/share/i)
    mounted.unmount()
  })
})
