import { beforeEach, describe, expect, it, vi } from 'vitest'

const toPng = vi.fn()
const commands = {
  saveImage: vi.fn(),
  deckScreenshotData: vi.fn(),
  leagueScreenshotData: vi.fn(),
  cardImageDataUrl: vi.fn(),
}

vi.mock('html-to-image', () => ({ toPng }))
vi.mock('@/bindings', () => ({ commands }))
vi.mock('@/lib/manaFontEmbed', () => ({ manaFontEmbedCss: async () => '@font-face { Mana }' }))

const { useOffscreenScreenshot, loadDeckScreenshot, loadLeagueCover, loadCardImage, imagesReady } = await import(
  './useOffscreenScreenshot'
)
const { useToast } = await import('./useToast')

const DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

function lastToast() {
  const { toasts } = useToast()
  return toasts.value[toasts.value.length - 1]
}

/** An element whose images are already loaded. */
function element(): HTMLElement {
  return { querySelectorAll: () => [] } as unknown as HTMLElement
}

const refuse = async () => {
  throw new Error('NotAllowedError')
}

describe('off screen screenshots (0.x Decklist, LeagueCard, LeagueRunCard, DeckCardStats)', () => {
  beforeEach(() => {
    toPng.mockReset()
    for (const command of Object.values(commands)) command.mockReset()
    useToast().toasts.value = []
  })

  it('renders the data, captures the element, then clears it', async () => {
    toPng.mockResolvedValue(DATA_URL)
    const written: Blob[] = []
    const shot = useOffscreenScreenshot<{ name: string }>({ writeImage: async (blob) => void written.push(blob) })
    const target = element()
    let shownWhileCapturing: unknown = undefined
    toPng.mockImplementation(async () => {
      shownWhileCapturing = shot.shot.value
      return DATA_URL
    })
    shot.target.value = { $el: target }
    await shot.take(async () => ({ name: 'Qburn' }), 'qburn-deck.png')
    expect(toPng).toHaveBeenCalledWith(target, {
      pixelRatio: 2,
      includeQueryParams: true,
      fontEmbedCSS: '@font-face { Mana }',
    })
    expect(shownWhileCapturing).toEqual({ name: 'Qburn' })
    expect(written).toHaveLength(1)
    expect(lastToast()).toMatchObject({ type: 'success', title: 'Copied!', message: 'Screenshot copied to clipboard' })
    expect(shot.shot.value).toBeNull()
    expect(shot.busy.value).toBe(false)
  })

  it('offers "Save image" when the clipboard refuses', async () => {
    toPng.mockResolvedValue(DATA_URL)
    const shot = useOffscreenScreenshot<string>({ writeImage: refuse })
    shot.target.value = element()
    await shot.take(async () => 'q', 'qbolt-card.png')
    expect(lastToast()).toMatchObject({ type: 'error', title: 'Copy failed', message: 'Could not write to clipboard' })
    expect(shot.saveOffered.value).toBe(true)
    commands.saveImage.mockResolvedValue({ status: 'ok', data: true })
    await shot.save()
    expect(commands.saveImage.mock.calls[0][1]).toBe('qbolt-card.png')
    expect(shot.saveOffered.value).toBe(false)
  })

  it('says "Screenshot failed" when the capture fails', async () => {
    toPng.mockRejectedValue(new Error('q'))
    vi.spyOn(console, 'error').mockImplementation(() => undefined)
    const shot = useOffscreenScreenshot<string>()
    shot.target.value = element()
    await shot.take(async () => 'q', 'q.png')
    expect(lastToast()).toMatchObject({ type: 'error', title: 'Screenshot failed', message: 'Could not capture the element' })
  })

  it('stops without capturing when the data is missing, and runs one at a time', async () => {
    const shot = useOffscreenScreenshot<string>()
    shot.target.value = element()
    await shot.take(async () => null, 'q.png')
    expect(toPng).not.toHaveBeenCalled()

    toPng.mockResolvedValue(DATA_URL)
    let release: (value: string) => void = () => undefined
    const first = shot.take(() => new Promise<string>((resolve) => (release = resolve)), 'a.png')
    await shot.take(async () => 'b', 'b.png')
    release('a')
    await first
    expect(toPng).toHaveBeenCalledTimes(2) // one capture: a second pass confirms the first
  })

  it('loads the deck data, or says "Could not load deck data"', async () => {
    commands.deckScreenshotData.mockResolvedValue({ status: 'ok', data: { name: 'Qburn' } })
    expect(await loadDeckScreenshot(5)).toEqual({ name: 'Qburn' })
    expect(commands.deckScreenshotData).toHaveBeenCalledWith(5)

    commands.deckScreenshotData.mockResolvedValue({ status: 'error', error: { kind: 'refused', message: 'q' } })
    expect(await loadDeckScreenshot(5)).toBeNull()
    expect(lastToast()).toMatchObject({ type: 'error', title: 'Screenshot failed', message: 'Could not load deck data' })
  })

  it('goes on without the league cover or card image when they cannot be read', async () => {
    commands.leagueScreenshotData.mockResolvedValue({ status: 'ok', data: { coverArtBase64: 'data:image/jpeg;base64,cQ==' } })
    expect(await loadLeagueCover(5)).toBe('data:image/jpeg;base64,cQ==')
    expect(commands.leagueScreenshotData).toHaveBeenCalledWith(5)
    commands.leagueScreenshotData.mockRejectedValue(new Error('q'))
    expect(await loadLeagueCover(5)).toBeNull()
    expect(await loadLeagueCover(null)).toBeNull()

    commands.cardImageDataUrl.mockResolvedValue({ status: 'ok', data: 'data:image/jpeg;base64,cg==' })
    expect(await loadCardImage('q-oracle')).toBe('data:image/jpeg;base64,cg==')
    expect(commands.cardImageDataUrl).toHaveBeenCalledWith('q-oracle')
    commands.cardImageDataUrl.mockResolvedValue({ status: 'ok', data: null })
    expect(await loadCardImage('q-oracle')).toBeNull()
    commands.cardImageDataUrl.mockReset()
    expect(await loadCardImage(null)).toBeNull()
    expect(commands.cardImageDataUrl).not.toHaveBeenCalled()
    expect(useToast().toasts.value).toEqual([])
  })

  it('waits for images still loading', async () => {
    const listeners: Record<string, () => void> = {}
    const image = {
      complete: false,
      addEventListener: (name: string, listener: () => void) => (listeners[name] = listener),
      decode: vi.fn().mockResolvedValue(undefined),
    }
    const root = { querySelectorAll: () => [image] } as unknown as HTMLElement
    let done = false
    const waiting = imagesReady(root).then(() => (done = true))
    await Promise.resolve()
    expect(done).toBe(false)
    listeners.load()
    await waiting
    expect(image.decode).toHaveBeenCalled()
  })
})
