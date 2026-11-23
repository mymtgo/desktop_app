import manaCss from 'mana-font/css/mana.css?raw'
import manaWoff2Url from 'mana-font/fonts/mana.woff2?url'
import { describe, expect, it } from 'vitest'
import { manaCostTokens, manaSymbolClass } from './mana'

describe('manaCostTokens (the website ManaCost.vue)', () => {
  it('keeps order and repeats', () => {
    expect(manaCostTokens('{2}{U}{U}')).toEqual(['2', 'U', 'U'])
  })

  it('reads hybrid, twobrid, Phyrexian, X, snow, colourless and big generics', () => {
    expect(manaCostTokens('{X}{U/R}{2/W}{B/P}{S}{C}{12}')).toEqual(['X', 'U/R', '2/W', 'B/P', 'S', 'C', '12'])
  })

  it('skips the text between the halves of a split card, as the website does', () => {
    expect(manaCostTokens('{1}{R} // {2}{U}')).toEqual(['1', 'R', '2', 'U'])
  })

  it('gives nothing for no cost and for a land', () => {
    expect(manaCostTokens(null)).toEqual([])
    expect(manaCostTokens('')).toEqual([])
  })
})

describe('manaSymbolClass', () => {
  it('maps tokens to mana-font cost classes', () => {
    expect(manaSymbolClass('U')).toBe('ms ms-u ms-cost')
    expect(manaSymbolClass('C')).toBe('ms ms-c ms-cost')
    expect(manaSymbolClass('10')).toBe('ms ms-10 ms-cost')
    expect(manaSymbolClass('X')).toBe('ms ms-x ms-cost')
    expect(manaSymbolClass('S')).toBe('ms ms-s ms-cost')
    expect(manaSymbolClass('U/R')).toBe('ms ms-ur ms-cost')
    expect(manaSymbolClass('2/W')).toBe('ms ms-2w ms-cost')
    expect(manaSymbolClass('B/P')).toBe('ms ms-bp ms-cost')
    expect(manaSymbolClass('G/U/P')).toBe('ms ms-gup ms-cost')
  })

  it('names only classes the bundled mana-font defines', () => {
    for (const token of ['W', 'U', 'B', 'R', 'G', 'C', 'X', 'S', '0', '1', '16', 'U/R', 'W/U', '2/B', 'B/P', 'R/G/P']) {
      const symbol = manaSymbolClass(token).split(' ')[1]
      expect(manaCss, symbol).toContain(`.${symbol}::before`)
    }
  })
})

describe('the bundled mana-font package (vite.config.ts manaFontWoff2Only)', () => {
  it('still has the Mana and MPlantin faces the build swaps for one woff2 face', () => {
    const faces = manaCss.match(/@font-face\s*\{[^}]*\}/g) ?? []
    expect(faces.map((face) => /font-family:\s*"?([^";]+)"?/.exec(face)?.[1])).toEqual(['Mana', 'MPlantin'])
    expect(manaCss.replace(/@font-face\s*\{[^}]*\}/g, '')).not.toContain('url(')
  })

  it('ships fonts/mana.woff2', () => {
    expect(manaWoff2Url).toMatch(/mana\.woff2$/)
  })
})
