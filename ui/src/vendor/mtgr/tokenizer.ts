import type { Id } from './types.js'

export type Token =
  | { t: 'string'; v: string }
  | { t: 'word'; v: string }
  | { t: 'id'; v: Id }
  | { t: 'int'; v: number; negative: boolean }
  | { t: 'pt'; power: number; toughness: number }
  | { t: 'symbols'; v: string[]; raw: string }
  | { t: 'nag'; v: number }
  | { t: 'brace'; v: string }
  | { t: 'comment'; v: string }

export class SyntaxError extends Error {}

const INT = /^-?(0|[1-9][0-9]*)$/
const DIGITS = /^-?[0-9]+$/
const PT = /^(-?[0-9]+)\/(-?[0-9]+)$/
const ID = /^#(h?)([1-9][0-9]*)$/
const SYMBOL_RUN = /^(\{[WUBRGCSP0-9/]+\})+$/

/** Splits the part of a body line after TIME and INDENT. Tokens are separated by exactly one space. */
export function tokenize(text: string): Token[] {
  const tokens: Token[] = []
  let i = 0
  while (i < text.length) {
    const c = text[i]
    if (c === ' ') throw new SyntaxError('two spaces between tokens')
    if (c === '"') {
      const [value, end] = readString(text, i)
      tokens.push({ t: 'string', v: value })
      i = end
    } else if (c === ';') {
      tokens.push({ t: 'comment', v: text.slice(i + 1) })
      i = text.length
    } else if (c === '{') {
      const space = text.indexOf(' ', i)
      const candidate = space === -1 ? text.slice(i) : text.slice(i, space)
      if (SYMBOL_RUN.test(candidate)) {
        tokens.push({ t: 'symbols', v: candidate.slice(1, -1).split('}{'), raw: candidate })
        i += candidate.length
      } else {
        const close = text.indexOf('}', i)
        if (close === -1) throw new SyntaxError('brace comment is not closed')
        tokens.push({ t: 'brace', v: text.slice(i + 1, close) })
        i = close + 1
      }
    } else {
      const space = text.indexOf(' ', i)
      const word = space === -1 ? text.slice(i) : text.slice(i, space)
      tokens.push(classify(word))
      i += word.length
    }
    if (i < text.length) {
      if (text[i] !== ' ') throw new SyntaxError(`expected a space after ${JSON.stringify(text.slice(i - 1, i + 1))}`)
      i += 1
      if (i === text.length) throw new SyntaxError('trailing space')
    }
  }
  return tokens
}

function classify(word: string): Token {
  if (word.startsWith('#')) {
    const m = ID.exec(word)
    if (!m) throw new SyntaxError(`bad instance ID ${word}`)
    return { t: 'id', v: { n: Number(m[2]), hypothetical: m[1] === 'h' } }
  }
  if (word.startsWith('$')) {
    const n = word.slice(1)
    if (!INT.test(n) || n.startsWith('-') || Number(n) > 255) throw new SyntaxError(`bad NAG ${word}`)
    return { t: 'nag', v: Number(n) }
  }
  if (DIGITS.test(word)) {
    if (!INT.test(word)) throw new SyntaxError(`integer with a leading zero: ${word}`)
    return { t: 'int', v: Number(word), negative: word.startsWith('-') }
  }
  const pt = PT.exec(word)
  if (pt) {
    if (!INT.test(pt[1]!) || !INT.test(pt[2]!)) throw new SyntaxError(`bad power/toughness ${word}`)
    return { t: 'pt', power: Number(pt[1]), toughness: Number(pt[2]) }
  }
  return { t: 'word', v: word }
}

/** Reads a quoted string starting at `start`; returns the decoded value and the index after the closing quote. */
export function readString(text: string, start: number): [string, number] {
  let out = ''
  let i = start + 1
  while (i < text.length) {
    const c = text[i]!
    if (c === '"') return [out, i + 1]
    if (c === '\\') {
      const e = text[i + 1]
      if (e === '"' || e === '\\') out += e
      else if (e === 'n') out += '\n'
      else throw new SyntaxError(`bad escape \\${e ?? ''} in string`)
      i += 2
    } else {
      out += c
      i += 1
    }
  }
  throw new SyntaxError('string is not closed')
}
