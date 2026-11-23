/**
 * Mana symbols drawn with the bundled mana-font (issue #326), as the
 * mymtgo.com website draws them (`ManaCost.vue`).
 */

/**
 * A Scryfall mana cost (`{1}{U}{R}`, `{U/R}` hybrid, `{2/W}` twobrid,
 * `{U/P}` Phyrexian, `{X}`, `{S}` snow) as its tokens, in order with repeats:
 * `{1}{U}{U}` is three tokens. Text outside braces (a split card's ` // `) is
 * skipped, as on the website. Null (a cost never stored) and empty (a land)
 * give none.
 */
export function manaCostTokens(cost: string | null): string[] {
  if (!cost) return []
  return [...cost.matchAll(/\{([^}]+)\}/g)].map((match) => match[1])
}

/**
 * A token as mana-font classes: lowercase, slash removed, drawn as a cost
 * disc. `{U/R}` is `ms-ur`, `{2/W}` `ms-2w`, `{B/P}` `ms-bp`, `{10}` `ms-10`.
 * mana-font names each hybrid in Scryfall's order only (`ms-ur`, not `ms-ru`).
 */
export function manaSymbolClass(token: string): string {
  return `ms ms-${token.toLowerCase().replace(/\//g, '')} ms-cost`
}

/** The sizes a row of symbols comes in: the disc's width in pixels. */
export type ManaSize = 'xs' | 'sm' | 'md' | 'lg'

/**
 * The font size for a disc size. A cost disc is 1.3em at 0.95em of the
 * font size (1.235 times it), so 13px (the website's size) draws a 16px disc.
 */
export const MANA_FONT_SIZES: Record<ManaSize, string> = {
  xs: 'text-[9.75px]', // 12px
  sm: 'text-[11.25px]', // 14px
  md: 'text-[13px]', // 16px
  lg: 'text-[14.5px]', // 18px
}

// ---- Colours ------------------------------------------------------------------

/** The colour names by identity letter, WUBRG then colourless (the order colours are listed in). */
export const MANA_COLOR_NAMES: Record<string, string> = {
  W: 'White',
  U: 'Blue',
  B: 'Black',
  R: 'Red',
  G: 'Green',
  C: 'Colorless',
}

/**
 * The five mana colours, plus colourless. Anything tinting a surface by
 * colour identity reads from the same palette.
 */
export const MANA_COLORS: Record<string, string> = {
  W: '#F8F6D8',
  U: '#C1D7E9',
  B: '#BAB1AB',
  R: '#E49977',
  G: '#A3C095',
  C: '#CCC2C0',
}
