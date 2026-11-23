import { describe, expect, it } from 'vitest'
import type { ListCardView, TypeGroupView } from '@/bindings'
import {
  cardCount,
  colourStats,
  curveBuckets,
  curveMax,
  decklistOrgUrl,
  dekExportFailedToast,
  dekExportToast,
  drawOddsCards,
  groupedMaindeck,
  openRegistrationLink,
  cardRowKey,
  resultLines,
} from './decklist'

function card(overrides: Partial<ListCardView> = {}): ListCardView {
  return {
    catalogId: 1,
    name: 'Qlightning Bolt',
    typeLine: 'Instant',
    colors: ['R'],
    image: 'https://cards.scryfall.io/normal/front/a/b/ab.jpg',
    localImage: null,
    artCrop: null,
    localArtCrop: null,
    cmc: 1,
    manaCost: '{R}',
    quantity: 4,
    sideboard: false,
    known: true,
    ...overrides,
  }
}

function group(typeName: string, cards: ListCardView[]): TypeGroupView {
  return { typeName, count: cardCount(cards), cards }
}

describe('cardCount', () => {
  it('adds the quantities', () => {
    expect(cardCount([card({ quantity: 4 }), card({ quantity: 2 })])).toBe(6)
    expect(cardCount([])).toBe(0)
  })
})

describe('groupedMaindeck', () => {
  it('merges groups by canonical type and orders them as 0.x', () => {
    const creature = card({ name: 'Qgoyf', typeLine: 'Creature' })
    const artifactCreature = card({ name: 'Qconstruct', typeLine: 'Artifact Creature' })
    const land = card({ name: 'Qmountain', typeLine: 'Land' })
    const unknown = card({ name: 'Card 99', typeLine: null, known: false, image: null, localImage: null })
    const groups = groupedMaindeck([
      group('Land', [land]),
      group('Unknown', [unknown]),
      group('Creature', [creature]),
      group('Artifact Creature', [artifactCreature]),
    ])
    expect(groups.map((g) => g.type)).toEqual(['Creature', 'Land', 'Unknown'])
    expect(groups[0].cards.map((c) => c.name)).toEqual(['Qgoyf', 'Qconstruct'])
    expect(groups[0].count).toBe(8)
  })

  it('keeps the order of types it does not know', () => {
    const groups = groupedMaindeck([group('Conspiracy', [card()]), group('Unknown', [card()])])
    expect(groups.map((g) => g.type)).toEqual(['Conspiracy', 'Unknown'])
  })
})

describe('decklistOrgUrl', () => {
  it('builds the registration link from the main deck, sideboard and format', () => {
    const main = [
      group('Creature', [card({ name: 'Qgoyf', quantity: 4 })]),
      group('Land', [card({ name: 'Qmountain', quantity: 20 })]),
    ]
    const side = [card({ name: "Qkor Firewalker's Ward", quantity: 2, sideboard: true })]
    const url = decklistOrgUrl(main, side, 'Modern')
    expect(url).toBe(
      'https://decklist.org/?deckmain=4+Qgoyf%0A20+Qmountain&deckside=2+Qkor+Firewalker%27s+Ward&eventformat=Modern',
    )
    const params = new URL(url).searchParams
    expect(params.get('deckmain')).toBe('4 Qgoyf\n20 Qmountain')
    expect(params.get('deckside')).toBe("2 Qkor Firewalker's Ward")
    expect(params.get('eventformat')).toBe('Modern')
  })

  it('leaves out cards without card data, as 0.x never listed them', () => {
    const main = [group('Unknown', [card({ name: 'Card 99', known: false })])]
    const side = [card({ name: 'Card 98', known: false, sideboard: true })]
    expect(decklistOrgUrl(main, side, 'Pauper')).toBe('https://decklist.org/?deckmain=&deckside=&eventformat=Pauper')
  })
})

describe('drawOddsCards', () => {
  it('lists every main deck card with its quantity', () => {
    const main = [
      group('Creature', [card({ name: 'Qgoyf', quantity: 4 })]),
      group('Unknown', [card({ name: 'Card 99', quantity: 1, known: false })]),
    ]
    expect(drawOddsCards(main)).toEqual([
      { name: 'Qgoyf', quantity: 4 },
      { name: 'Card 99', quantity: 1 },
    ])
  })

  it('adds up printings that share a name', () => {
    const main = [
      group('Land', [card({ catalogId: 1, name: 'Qmountain', quantity: 10 }), card({ catalogId: 2, name: 'Qmountain', quantity: 8 })]),
    ]
    expect(drawOddsCards(main)).toEqual([{ name: 'Qmountain', quantity: 18 }])
  })
})

describe('resultLines', () => {
  it('labels the four odds with the wanted count, as 0.x', () => {
    const lines = resultLines({ atLeast: 0.4, exactly: 0.3, atMost: 0.9, zero: 0.6 }, 1)
    expect(lines).toEqual([
      { label: 'Chance to draw 1 or more', value: '40.0%' },
      { label: 'Chance to draw exactly 1', value: '30.0%' },
      { label: 'Chance to draw 1 or less', value: '90.0%' },
      { label: 'Chance to draw 0', value: '60.0%' },
    ])
  })

  it('shows a negative wanted count as 0', () => {
    expect(resultLines({ atLeast: 1, exactly: 0, atMost: 0, zero: 0 }, -2)[0].label).toBe('Chance to draw 0 or more')
  })
})

describe('curveBuckets', () => {
  it('labels mana values, 7 being 7 and up', () => {
    const buckets = curveBuckets([
      { cmc: 1, count: 6 },
      { cmc: 2, count: 4 },
      { cmc: 7, count: 3 },
    ])
    expect(buckets).toEqual([
      { cmc: '1', count: 6 },
      { cmc: '2', count: 4 },
      { cmc: '7+', count: 3 },
    ])
    expect(curveMax(buckets)).toBe(6)
  })

  it('has a maximum of at least 1', () => {
    expect(curveMax([])).toBe(1)
  })
})

describe('colourStats', () => {
  it('names each colour and rounds its share of the nonland cards', () => {
    const stats = colourStats(
      [
        { colour: 'U', count: 10 },
        { colour: 'R', count: 7 },
        { colour: 'C', count: 2 },
      ],
      15,
    )
    expect(stats).toEqual([
      { color: 'U', label: 'Blue', count: 10, total: 15, percentage: 67 },
      { color: 'R', label: 'Red', count: 7, total: 15, percentage: 47 },
      { color: 'C', label: 'Colorless', count: 2, total: 15, percentage: 13 },
    ])
  })

  it('is empty without nonland cards', () => {
    expect(colourStats([{ colour: 'U', count: 1 }], 0)).toEqual([])
  })
})

describe('dekExportToast', () => {
  it('names the saved file', () => {
    expect(dekExportToast({ kind: 'saved', path: 'C:\\Decks\\Qdeck.dek' })).toEqual({
      type: 'success',
      title: 'Saved deck file',
      message: 'C:\\Decks\\Qdeck.dek',
    })
  })

  it('says nothing when the dialog was cancelled', () => {
    expect(dekExportToast({ kind: 'cancelled' })).toBeNull()
  })

  it('reports a failure with its message', () => {
    expect(dekExportFailedToast(new Error('This deck has no cards to export.'))).toEqual({
      type: 'error',
      title: 'Export failed',
      message: 'This deck has no cards to export.',
    })
  })
})

describe('opening the registration link', () => {
  it('opens it in the browser through the app, not the webview', async () => {
    const opened: string[] = []
    const open = async (url: string) => {
      opened.push(url)
      return { status: 'ok' as const, data: null }
    }
    expect(await openRegistrationLink('https://decklist.org/?a=1', open)).toBeNull()
    expect(opened).toEqual(['https://decklist.org/?a=1'])
  })

  it('reports a refused or failed open with its message', async () => {
    const open = async () => ({
      status: 'error' as const,
      error: { kind: 'refused' as const, message: 'This link cannot be opened.' },
    })
    expect(await openRegistrationLink('https://evil.com/', open)).toEqual({
      type: 'error',
      title: 'Could not open the link',
      message: 'This link cannot be opened.',
    })
  })
})

describe('card row keys', () => {
  it('stay unique when a list repeats a printing (signature rows are not merged)', () => {
    const rows = [card({ catalogId: 7 }), card({ catalogId: 7 }), card({ catalogId: 8 })]
    const keys = rows.map((row, index) => cardRowKey(row, index))
    expect(new Set(keys).size).toBe(3)
  })
})
