// A deck's own cover image is shown cropped to a landscape box, like the
// cover cards' art crops (626 x 457). An image larger than twice that is
// cropped to its shape before it is stored.

export const COVER_WIDTH = 1252
export const COVER_HEIGHT = 914

export type Size = { width: number; height: number }
export type Offset = { x: number; y: number }
export type Rect = Offset & Size

/** Whether an image this size is cropped before it is stored. */
export function needsCrop(image: Size): boolean {
  return image.width > COVER_WIDTH || image.height > COVER_HEIGHT
}

/** The smallest scale at which the image covers the frame. */
export function coverScale(image: Size, frame: Size): number {
  return Math.max(frame.width / image.width, frame.height / image.height)
}

/** The image's top-left corner in the frame, kept so the image covers it. */
export function clampOffset(offset: Offset, image: Size, scale: number, frame: Size): Offset {
  const clamp = (value: number, shown: number, room: number) => Math.min(0, Math.max(room - shown, value))
  return {
    x: clamp(offset.x, image.width * scale, frame.width),
    y: clamp(offset.y, image.height * scale, frame.height),
  }
}

/** The image centred in the frame at `scale`. */
export function centredOffset(image: Size, scale: number, frame: Size): Offset {
  return {
    x: (frame.width - image.width * scale) / 2,
    y: (frame.height - image.height * scale) / 2,
  }
}

/** The part of the image the frame shows, in image pixels. */
export function sourceRect(offset: Offset, scale: number, frame: Size): Rect {
  return {
    x: -offset.x / scale,
    y: -offset.y / scale,
    width: frame.width / scale,
    height: frame.height / scale,
  }
}

/** The stored size of a crop: the cover size, or smaller when the crop is. */
export function outputSize(source: Size): Size {
  const width = Math.max(1, Math.min(COVER_WIDTH, Math.round(source.width)))
  return { width, height: Math.round((width * COVER_HEIGHT) / COVER_WIDTH) }
}

/** How a crop is encoded: PNG keeps a PNG's transparency, anything else is JPEG. */
export function outputType(mime: string): { mime: string; extension: string } {
  return mime === 'image/png' ? { mime: 'image/png', extension: 'png' } : { mime: 'image/jpeg', extension: 'jpg' }
}

/** A file name's extension, lower case (`''` when it has none). */
export function extensionOf(name: string): string {
  const dot = name.lastIndexOf('.')
  return dot < 0 ? '' : name.slice(dot + 1).toLowerCase()
}
