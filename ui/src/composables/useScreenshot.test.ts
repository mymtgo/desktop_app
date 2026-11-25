import manifestText from '../../package.json?raw'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const toPng = vi.fn()
const commands = { saveImage: vi.fn() }

vi.mock('html-to-image', () => ({ toPng }))
vi.mock('@/bindings', () => ({ commands }))
vi.mock('@/lib/manaFontEmbed', () => ({ manaFontEmbedCss: async () => '@font-face { Mana }' }))

const { useScreenshot, dataUrlBytes } = await import('./useScreenshot')
const { useToast } = await import('./useToast')

/** A 1x1 PNG as html-to-image returns it. */
const DATA_URL =
  'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg=='

/** A data URL whose length grows with `size`. */
const pngOfSize = (size: number) => `data:image/png;base64,${btoa('x'.repeat(size * 3))}`

/** A stand-in element that records its attributes. */
function fakeElement() {
  const attributes = new Set<string>()
  return {
    attributes,
    setAttribute: (name: string) => void attributes.add(name),
    removeAttribute: (name: string) => void attributes.delete(name),
  }
}

const element = fakeElement() as unknown as HTMLElement

function lastToast() {
  const { toasts } = useToast()
  return toasts.value[toasts.value.length - 1]
}

describe('useScreenshot (0.x useScreenshot, spec section 9)', () => {
  beforeEach(() => {
    toPng.mockReset()
    commands.saveImage.mockReset()
    useToast().toasts.value = []
  })

  it('renders at pixel ratio 2 and copies the PNG', async () => {
    toPng.mockResolvedValue(DATA_URL)
    const written: Blob[] = []
    const shot = useScreenshot({ writeImage: async (blob) => void written.push(blob) })
    await shot.capture(element, 'modern-metagame-last30')
    expect(toPng).toHaveBeenCalledWith(element, {
      pixelRatio: 2,
      includeQueryParams: true,
      fontEmbedCSS: '@font-face { Mana }',
    })
    expect(written).toHaveLength(1)
    expect(written[0].type).toBe('image/png')
    expect(lastToast()).toMatchObject({ type: 'success', title: 'Copied!', message: 'Screenshot copied to clipboard' })
    expect(shot.saveOffered.value).toBe(false)
    expect(shot.capturing.value).toBe(false)
  })

  it('marks the element as capturing only while it renders, so a watermark can show', async () => {
    const target = fakeElement()
    const seen: boolean[] = []
    toPng.mockImplementation(async () => {
      seen.push(target.attributes.has('data-capturing'))
      return DATA_URL
    })
    const shot = useScreenshot({ writeImage: async () => {} })
    await shot.capture(target as unknown as HTMLElement, 'wrapped')
    expect(seen.length).toBeGreaterThan(0)
    expect(seen.every(Boolean)).toBe(true)
    expect(target.attributes.has('data-capturing')).toBe(false)

    toPng.mockRejectedValue(new Error('draw failed'))
    await shot.capture(target as unknown as HTMLElement, 'wrapped')
    expect(target.attributes.has('data-capturing')).toBe(false)
  })

  it('captures again until the result stops changing and keeps the last', async () => {
    for (const size of [1, 2, 3, 3]) toPng.mockResolvedValueOnce(pngOfSize(size))
    const written: Blob[] = []
    const shot = useScreenshot({ writeImage: async (blob) => void written.push(blob) })
    await shot.capture(element, 'deck')
    expect(toPng).toHaveBeenCalledTimes(4)
    expect(written).toHaveLength(1)
  })

  it('stops after two passes when the first two agree', async () => {
    toPng.mockResolvedValue(DATA_URL)
    const shot = useScreenshot({ writeImage: async () => {} })
    await shot.capture(element, 'deck')
    expect(toPng).toHaveBeenCalledTimes(2)
  })

  it('caps the passes when the result never settles', async () => {
    let size = 0
    toPng.mockImplementation(async () => pngOfSize(++size))
    const shot = useScreenshot({ writeImage: async () => {} })
    await shot.capture(element, 'deck')
    expect(toPng).toHaveBeenCalledTimes(4)
    expect(lastToast()).toMatchObject({ type: 'success' })
  })

  it('reports a failure in a later pass', async () => {
    toPng.mockResolvedValueOnce(DATA_URL).mockRejectedValueOnce(new Error('boom'))
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const shot = useScreenshot({ writeImage: async () => {} })
    await shot.capture(element, 'deck')
    expect(lastToast()).toMatchObject({ type: 'error', title: 'Screenshot failed' })
    spy.mockRestore()
  })

  it('offers "Save image" when the clipboard refuses, and saves through the dialog', async () => {
    toPng.mockResolvedValue(DATA_URL)
    const shot = useScreenshot({
      writeImage: async () => {
        throw new Error('NotAllowedError')
      },
    })
    await shot.capture(element, 'modern-metagame-last30')
    expect(lastToast()).toMatchObject({ type: 'error', title: 'Copy failed', message: 'Could not write to clipboard' })
    expect(shot.saveOffered.value).toBe(true)

    commands.saveImage.mockResolvedValue({ status: 'ok', data: true })
    await shot.save()
    expect(commands.saveImage).toHaveBeenCalledWith(Array.from(dataUrlBytes(DATA_URL)), 'modern-metagame-last30')
    expect(lastToast()).toMatchObject({ type: 'success', title: 'Saved', message: 'Image saved' })
    expect(shot.saveOffered.value).toBe(false)
  })

  it('keeps the offer when the save dialog is cancelled', async () => {
    toPng.mockResolvedValue(DATA_URL)
    const shot = useScreenshot({
      writeImage: async () => {
        throw new Error('refused')
      },
    })
    await shot.capture(element, 'x')
    commands.saveImage.mockResolvedValue({ status: 'ok', data: false })
    await shot.save()
    expect(shot.saveOffered.value).toBe(true)
  })

  it('says the capture failed when rendering fails', async () => {
    toPng.mockRejectedValue(new Error('tainted canvas'))
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {})
    const shot = useScreenshot({ writeImage: async () => {} })
    await shot.capture(element, 'x')
    expect(lastToast()).toMatchObject({ type: 'error', title: 'Screenshot failed', message: 'Could not capture the element' })
    expect(shot.saveOffered.value).toBe(false)
    expect(shot.capturing.value).toBe(false)
    errors.mockRestore()
  })

  it('decodes the data URL to the PNG bytes', () => {
    const bytes = dataUrlBytes(DATA_URL)
    expect(Array.from(bytes.slice(0, 4))).toEqual([0x89, 0x50, 0x4e, 0x47])
  })
})

describe('the html-to-image pin (owner approval 2026-10-09)', () => {
  it('is exactly 1.11.13', () => {
    const manifest = JSON.parse(manifestText)
    expect(manifest.dependencies['html-to-image']).toBe('1.11.13')
  })
})
