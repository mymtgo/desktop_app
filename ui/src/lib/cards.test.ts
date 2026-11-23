import { describe, expect, it, vi } from 'vitest'
import { nextTick, reactive } from 'vue'
import {
  MISSING_FOLDER_TEXT,
  cardDataSummary,
  cardImageSrc,
  cardImagesPollInterval,
  CARD_TYPE_KEYS,
  cardsFromUrl,
  cardsUrlQuery,
  DEFAULT_CARDS,
  archetypeCountLabel,
  popularityTitle,
  downloadCardDataLabel,
  folderButtonLabel,
  hiddenTypesFrom,
  typeToggles,
  useCardImage,
} from './cards'

vi.mock('@tauri-apps/api/core', () => ({
  convertFileSrc: (path: string, protocol: string) => `${protocol}://localhost/${encodeURIComponent(path)}`,
}))

describe('card data summary', () => {
  it('says nothing is downloaded when no card is stored', () => {
    expect(cardDataSummary({ total: 0, incomplete: 0 })).toBe(
      'No card details downloaded yet. Decks and matches will show without names or artwork until you download them.',
    )
  })

  it('counts the cards still missing details', () => {
    expect(cardDataSummary({ total: 120, incomplete: 7 })).toBe('120 cards stored, 7 still missing details.')
  })

  it('says every card has details when none is missing', () => {
    expect(cardDataSummary({ total: 120, incomplete: 0 })).toBe('120 cards stored, all with details.')
  })

  it('labels the button while downloading', () => {
    expect(downloadCardDataLabel(true)).toBe('Downloading...')
    expect(downloadCardDataLabel(false)).toBe('Download card data')
  })
})

describe('cardImageSrc', () => {
  it('picks the local file, then the remote URL', () => {
    expect(cardImageSrc('https://cards.scryfall.io/a.jpg', '12_art.jpg')).toBe('card://localhost/12_art.jpg')
    expect(cardImageSrc('https://cards.scryfall.io/a.jpg', null)).toBe('https://cards.scryfall.io/a.jpg')
    expect(cardImageSrc(null, null)).toBeNull()
  })
})

describe('CardImage', () => {
  it('falls back once to the remote URL when the local file fails', async () => {
    const props = reactive({ src: 'https://cards.scryfall.io/a.jpg' as string | null, local: '12.jpg' as string | null })
    const { current, onError } = useCardImage(props)
    expect(current.value).toBe('card://localhost/12.jpg')
    onError()
    expect(current.value).toBe('https://cards.scryfall.io/a.jpg')
    // A remote that fails too stays as it is: no loop back to the local file.
    onError()
    expect(current.value).toBe('https://cards.scryfall.io/a.jpg')
    // Another card's file is tried again.
    props.local = '13.jpg'
    await nextTick()
    expect(current.value).toBe('card://localhost/13.jpg')
  })

  it('shows the decks own cover first and falls back one step per load error', async () => {
    const props = reactive({
      src: 'https://cards.scryfall.io/a.jpg' as string | null,
      local: '12_art.jpg' as string | null,
      custom: '5-1700.png' as string | null,
    })
    const { current, onError } = useCardImage(props)
    expect(current.value).toBe('deckcover://localhost/5-1700.png')
    onError()
    expect(current.value).toBe('card://localhost/12_art.jpg')
    onError()
    expect(current.value).toBe('https://cards.scryfall.io/a.jpg')
    // A replaced cover is tried again.
    props.custom = '5-1800.png'
    await nextTick()
    expect(current.value).toBe('deckcover://localhost/5-1800.png')
  })

  it('shows the decks own cover when the printing has no art at all', () => {
    const { current } = useCardImage({ src: null, local: null, custom: '5-1700.png' })
    expect(current.value).toBe('deckcover://localhost/5-1700.png')
  })

  it('shows the remote URL when there is no local file', () => {
    const { current, onError } = useCardImage({ src: 'https://cards.scryfall.io/a.jpg', local: null })
    expect(current.value).toBe('https://cards.scryfall.io/a.jpg')
    onError()
    expect(current.value).toBe('https://cards.scryfall.io/a.jpg')
  })
})

describe('card images folder', () => {
  it('labels the folder button while a move runs', () => {
    expect(folderButtonLabel(true)).toBe('Moving...')
    expect(folderButtonLabel(false)).toBe('Change')
  })

  it('says the folder is gone and downloads wait', () => {
    expect(MISSING_FOLDER_TEXT).toBe(
      'Image folder not found. Downloaded images will not show until it is back, and new downloads are paused.',
    )
  })

  it('polls every second while a move runs, as 0.x', () => {
    const folder = { path: '/q', isDefault: true, missing: false, moving: true, error: null }
    expect(cardImagesPollInterval({ usage: '0 B', folder })).toBe(1000)
    expect(cardImagesPollInterval(undefined)).toBe(5000)
  })

  it('polls every 5 seconds otherwise, so the usage keeps up', () => {
    const folder = { path: '/q', isDefault: true, missing: false, moving: false, error: null }
    expect(cardImagesPollInterval({ usage: '0 B', folder })).toBe(5000)
  })
})

describe('Cards page query string', () => {
  it('reads the defaults from an empty query', () => {
    expect(cardsFromUrl({})).toEqual(DEFAULT_CARDS)
    expect(DEFAULT_CARDS.groupPrintings).toBe(true)
  })

  it('round trips every filter', () => {
    const state = {
      search: 'bolt',
      format: 'modern',
      hiddenTypes: ['Creature', 'Land'],
      groupPrintings: false,
      page: 3,
    }
    const query = cardsUrlQuery(state)
    expect(query).toEqual({
      search: 'bolt',
      format: 'modern',
      hidden_types: 'Creature,Land',
      group_printings: 'false',
      page: '3',
    })
    expect(cardsFromUrl(query)).toEqual(state)
  })

  it('leaves defaults out of the URL, as 0.x', () => {
    expect(cardsUrlQuery(DEFAULT_CARDS)).toEqual({})
  })

  it('keeps only real categories, in canonical order', () => {
    expect(cardsFromUrl({ hidden_types: 'Land,Nope,Creature,Other' }).hiddenTypes).toEqual(['Creature', 'Land'])
    expect(CARD_TYPE_KEYS).toHaveLength(8)
  })

  it('treats a bad page as the first', () => {
    expect(cardsFromUrl({ page: '0' }).page).toBe(1)
    expect(cardsFromUrl({ page: 'x' }).page).toBe(1)
  })
})

describe('Cards page words', () => {
  it('pluralises the popularity title and the sheet count', () => {
    expect(popularityTitle(1)).toBe('Used in 1 archetype')
    expect(popularityTitle(3)).toBe('Used in 3 archetypes')
    expect(archetypeCountLabel(1)).toBe('Seen in 1 archetype')
    expect(archetypeCountLabel(2)).toBe('Seen in 2 archetypes')
  })
})

describe('the card hover preview (0.x CardHoverPreview)', () => {
  it('shows only for a card with an image, after 0.x delays', async () => {
    const { hasCardPreview, PREVIEW_OPEN_DELAY_MS, PREVIEW_CLOSE_DELAY_MS } = await import('./cards')
    expect(hasCardPreview(null, null)).toBe(false)
    expect(hasCardPreview(null, undefined)).toBe(false)
    expect(hasCardPreview('https://img.example/1.jpg', null)).toBe(true)
    expect(hasCardPreview(null, '1.jpg')).toBe(true)
    expect([PREVIEW_OPEN_DELAY_MS, PREVIEW_CLOSE_DELAY_MS]).toEqual([150, 50])
  })
})

describe('Cards page type toggles', () => {
  it('shows every type on and the Sideboard toggle untouched when nothing is hidden', () => {
    const toggles = typeToggles([])
    expect(Object.values(toggles).every(Boolean)).toBe(true)
  })

  it('turns the hidden types off', () => {
    const toggles = typeToggles(['Land', 'Battle'])
    expect(toggles.Land).toBe(false)
    expect(toggles.Battle).toBe(false)
    expect(toggles.Creature).toBe(true)
  })

  it('reads the hidden types back in canonical order', () => {
    expect(hiddenTypesFrom({ ...typeToggles([]), Land: false, Creature: false })).toEqual(['Creature', 'Land'])
    expect(hiddenTypesFrom(typeToggles([]))).toEqual([])
  })
})
