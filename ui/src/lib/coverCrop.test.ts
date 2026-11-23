import { describe, expect, it } from 'vitest'
import {
  COVER_HEIGHT,
  COVER_WIDTH,
  centredOffset,
  clampOffset,
  coverScale,
  extensionOf,
  needsCrop,
  outputSize,
  outputType,
  sourceRect,
} from './coverCrop'

const frame = { width: 626, height: 457 }

describe('cover crop', () => {
  it('crops only an image larger than the cover size', () => {
    expect(needsCrop({ width: COVER_WIDTH, height: COVER_HEIGHT })).toBe(false)
    expect(needsCrop({ width: 800, height: 600 })).toBe(false)
    expect(needsCrop({ width: 1024, height: 1536 })).toBe(true)
    expect(needsCrop({ width: 1300, height: 400 })).toBe(true)
  })

  it('scales a portrait image to the frame width', () => {
    const image = { width: 1024, height: 1536 }
    expect(coverScale(image, frame)).toBeCloseTo(626 / 1024)
  })

  it('never lets the image leave a gap in the frame', () => {
    const image = { width: 1024, height: 1536 }
    const scale = coverScale(image, frame)
    expect(clampOffset({ x: 50, y: 50 }, image, scale, frame)).toEqual({ x: 0, y: 0 })
    const bottom = clampOffset({ x: -9999, y: -9999 }, image, scale, frame)
    expect(bottom.x).toBeCloseTo(0)
    expect(bottom.y).toBeCloseTo(457 - 1536 * scale)
  })

  it('reads the framed part back in image pixels', () => {
    const image = { width: 1024, height: 1536 }
    const scale = coverScale(image, frame)
    const rect = sourceRect(centredOffset(image, scale, frame), scale, frame)
    expect(rect.x).toBeCloseTo(0)
    expect(rect.width).toBeCloseTo(1024)
    expect(rect.height).toBeCloseTo((1024 * 457) / 626)
    expect(rect.y).toBeCloseTo((1536 - rect.height) / 2)
  })

  it('stores at the cover size, or the crop size when that is smaller', () => {
    expect(outputSize({ width: 3000, height: 2190 })).toEqual({ width: COVER_WIDTH, height: COVER_HEIGHT })
    expect(outputSize({ width: 1024, height: 747.6 })).toEqual({ width: 1024, height: 748 })
  })

  it('keeps PNG and turns everything else into JPEG', () => {
    expect(outputType('image/png')).toEqual({ mime: 'image/png', extension: 'png' })
    expect(outputType('image/webp')).toEqual({ mime: 'image/jpeg', extension: 'jpg' })
  })

  it('reads a lower-case extension', () => {
    expect(extensionOf('Dog.PNG')).toBe('png')
    expect(extensionOf('no-extension')).toBe('')
  })
})
