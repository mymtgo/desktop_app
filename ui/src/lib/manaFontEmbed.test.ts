import { beforeEach, describe, expect, it, vi } from 'vitest'
import { bytesToBase64, manaFontEmbedCss, manaFontFace, resetManaFontEmbed } from './manaFontEmbed'

describe('manaFontEmbed (screenshots draw mana symbols)', () => {
  beforeEach(() => {
    resetManaFontEmbed()
    vi.spyOn(console, 'error').mockImplementation(() => {})
  })

  it('writes base64, also past one chunk', () => {
    expect(bytesToBase64(new Uint8Array([77, 97, 110, 97]))).toBe('TWFuYQ==')
    const big = new Uint8Array(0x8000 * 2 + 5).fill(65)
    expect(atob(bytesToBase64(big))).toBe('A'.repeat(big.length))
  })

  it('declares the Mana face with the woff2 inline', () => {
    expect(manaFontFace(new Uint8Array([77, 97, 110, 97]))).toBe(
      "@font-face { font-family: 'Mana'; src: url(data:font/woff2;base64,TWFuYQ==) format('woff2'); font-weight: normal; font-style: normal; }",
    )
  })

  it('reads the font once', async () => {
    const read = vi.fn(async () => new Uint8Array([1]))
    const first = await manaFontEmbedCss(read)
    expect(await manaFontEmbedCss(read)).toBe(first)
    expect(first).toContain('base64,AQ==')
    expect(read).toHaveBeenCalledTimes(1)
  })

  it('gives no CSS when the font cannot be read, and tries again next time', async () => {
    const read = vi.fn(async (): Promise<Uint8Array> => {
      throw new Error('gone')
    })
    expect(await manaFontEmbedCss(read)).toBe('')
    expect(await manaFontEmbedCss(read)).toBe('')
    expect(read).toHaveBeenCalledTimes(2)
  })
})
