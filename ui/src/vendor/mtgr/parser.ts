import { Cursor, parseStatement, parseTime, type Context } from './statements.js'
import { SyntaxError, readString, tokenize } from './tokenizer.js'
import type { BodyLine, Game, Id, MtgrFile, ParseResult, Problem, Result, Statement, Tag } from './types.js'

const REQUIRED_TAGS = [
  'Mtgr',
  'Event',
  'Site',
  'Date',
  'Round',
  'Player1',
  'Player2',
  'Result',
  'Format',
  'Match',
  'Game',
  'Perspective',
]
const RESULTS = new Set<string>(['1-0', '0-1', '1/2-1/2', '*'])
/** Format versions this reader follows (section 4.2). */
const VERSIONS = new Set<string>(['0.3.0'])
const TAG_LINE = /^\[([A-Z][A-Za-z0-9_]*) (".*")\]$/

/** Parses a whole file. Never throws: syntax failures become check 1 problems and the line is dropped. */
export function parse(text: string): ParseResult {
  const problems: Problem[] = []
  const problem = (line: number, message: string) => problems.push({ line, check: 1, message })

  if (text.startsWith('﻿')) {
    problem(1, 'file starts with a byte-order mark')
    text = text.slice(1)
  }
  if (text.length > 0 && !text.endsWith('\n')) problem(text.split('\n').length, 'file does not end with LF')
  const lines = text.split('\n')
  if (lines[lines.length - 1] === '') lines.pop()

  const firstCr = lines.findIndex((l) => l.endsWith('\r'))
  if (firstCr !== -1) problem(firstCr + 1, 'lines end with CR LF; only LF is allowed')
  for (const [i, raw] of lines.entries()) {
    if (raw.normalize('NFC') !== raw) problem(i + 1, 'line is not in Unicode Normalization Form C')
    if (/[\u0000-\u0009\u000B-\u001F\u007F]/.test(raw.replace(/\r$/, ''))) problem(i + 1, 'line contains a tab or control character')
  }

  const games: Game[] = []
  const st: { section: 'tags' | 'body' | 'between' } = { section: 'tags' }
  let tags: Tag[] = []
  let body: BodyLine[] = []
  let nesting = new Nesting()
  let gameStart = 1

  const finishGame = (result: Result, resultLine: number) => {
    checkTags(tags, gameStart, problem)
    games.push({ tags, body, result, resultLine })
    tags = []
    body = []
    nesting = new Nesting()
    st.section = 'between'
  }

  for (const [i, raw] of lines.entries()) {
    const n = i + 1
    const line = raw.replace(/\r$/, '')
    if (st.section === 'between') {
      if (line === '') {
        st.section = 'tags'
        gameStart = n + 1
        continue
      }
      problem(n, 'expected one empty line between games')
      st.section = 'tags'
      gameStart = n
    }
    if (st.section === 'tags') {
      if (line === '') {
        if (tags.length === 0) problem(n, 'empty line before any tag')
        st.section = 'body'
        continue
      }
      if (line.startsWith('[')) {
        const tag = parseTag(line, n)
        if (typeof tag === 'string') problem(n, tag)
        else tags.push(tag)
        continue
      }
      if (line.startsWith('%')) {
        problem(n, 'escape line inside the tag section')
        continue
      }
      problem(n, 'expected an empty line between the tag section and the body')
      st.section = 'body'
    }
    if (line === '') {
      problem(n, 'empty line inside a game body')
      continue
    }
    if (line.startsWith('%')) {
      body.push({ kind: 'escape', line: n, text: line.slice(1) })
      continue
    }
    if (RESULTS.has(line)) {
      if (body.length === 0) problem(n, 'game body is empty')
      finishGame(line as Result, n)
      continue
    }
    try {
      body.push(parseBodyLine(line, n, nesting))
    } catch (e) {
      if (!(e instanceof SyntaxError)) throw e
      problem(n, e.message)
    }
  }
  if (st.section === 'tags' && tags.length > 0) problem(lines.length, 'file ends inside a tag section')
  if (st.section === 'body') {
    problem(lines.length, 'game has no result line')
    finishGame('*', lines.length)
  }
  const file: MtgrFile = { games }
  return { file, problems: problems.sort((a, b) => a.line - b.line) }
}

function parseTag(line: string, n: number): Tag | string {
  const m = TAG_LINE.exec(line)
  if (!m) return 'bad tag line'
  try {
    const [value, end] = readString(m[2]!, 0)
    if (end !== m[2]!.length) return 'bad tag value'
    return { name: m[1]!, value, line: n }
  } catch (e) {
    return e instanceof SyntaxError ? e.message : 'bad tag value'
  }
}

function checkTags(tags: Tag[], gameStart: number, problem: (line: number, message: string) => void) {
  for (const [i, required] of REQUIRED_TAGS.entries()) {
    const tag = tags[i]
    if (!tag) {
      problem(gameStart, `missing required tag ${required}`)
      return
    }
    if (tag.name !== required) {
      problem(tag.line, `expected tag ${required} here, got ${tag.name}`)
      return
    }
  }
  const seen = new Set<string>()
  for (const tag of tags) {
    if (seen.has(tag.name)) problem(tag.line, `tag ${tag.name} appears twice`)
    seen.add(tag.name)
  }
  const optional = tags.slice(REQUIRED_TAGS.length)
  for (let i = 1; i < optional.length; i++) {
    if (optional[i]!.name < optional[i - 1]!.name) {
      problem(optional[i]!.line, `tag ${optional[i]!.name} is out of ASCII order`)
    }
  }
  const value = (name: string) => tags.find((t) => t.name === name)!.value
  if (!VERSIONS.has(value('Mtgr'))) problem(tags[0]!.line, `unsupported format version ${value('Mtgr')}`)
  if (!RESULTS.has(value('Result'))) problem(tags[7]!.line, `bad Result value ${value('Result')}`)
  if (value('Player1') === value('Player2')) problem(tags[6]!.line, 'Player1 and Player2 are the same')
  const perspective = value('Perspective')
  if (perspective !== value('Player1') && perspective !== value('Player2')) {
    problem(tags[11]!.line, 'Perspective names neither Player1 nor Player2')
  }
}

interface Frame {
  depth: number
  children: Context
  variation: boolean
}

/** Child contexts per line kind (section 5.2); a line whose kind is absent takes no children. */
const CHILDREN: Partial<Record<Statement['type'], Context>> = {
  deck: 'deck',
  pregame: 'pregame',
  turn: 'turn',
  keyframe: 'keyframe',
  step: 'step',
  priority: 'priority',
  stack: 'stack',
}

/** Tracks the enclosing lines so each line parses in the context its parent allows. */
class Nesting {
  private frames: Frame[] = [{ depth: -1, children: 'top', variation: false }]

  private popTo(depth: number) {
    while (this.frames.length > 1 && this.frames[this.frames.length - 1]!.depth >= depth) this.frames.pop()
  }

  /** Context for a line at `depth`, after closing deeper frames. */
  enter(depth: number): { context: Context; variation: boolean } {
    this.popTo(depth)
    const parent = this.frames[this.frames.length - 1]!
    if (parent.depth !== depth - 1) {
      throw new SyntaxError(`line is indented ${depth} levels but its parent is at level ${parent.depth}`)
    }
    return { context: parent.children, variation: parent.variation }
  }

  push(depth: number, statement: Statement, variation: boolean) {
    const children = CHILDREN[statement.type]
    if (children) this.frames.push({ depth, children, variation })
  }

  openVariation(depth: number) {
    const { context } = this.enter(depth)
    this.frames.push({ depth, children: context, variation: true })
  }

  closeVariation(depth: number) {
    while (this.frames.length > 1 && this.frames[this.frames.length - 1]!.depth > depth) this.frames.pop()
    const top = this.frames[this.frames.length - 1]!
    if (!top.variation || top.depth !== depth) throw new SyntaxError('closing bracket without an open variation')
    this.frames.pop()
  }
}

function parseBodyLine(line: string, n: number, nesting: Nesting): BodyLine {
  const space = line.indexOf(' ')
  if (space === -1) throw new SyntaxError('expected a time, then a space')
  const timeText = line.slice(0, space)
  const time = timeText === '-' ? null : parseTime(timeText)
  if (time === null && timeText !== '-') throw new SyntaxError(`bad time ${timeText}`)
  let rest = line.slice(space + 1)
  let spaces = 0
  while (rest[spaces] === ' ') spaces++
  if (spaces % 2 !== 0) throw new SyntaxError(`indented ${spaces} spaces, which is no nesting level`)
  const depth = spaces / 2
  rest = rest.slice(spaces)

  if (rest === '(' || rest === ')') {
    if (timeText !== '-') throw new SyntaxError('variation brackets have time -')
    if (rest === '(') nesting.openVariation(depth)
    else nesting.closeVariation(depth)
    return { kind: 'variation', line: n, depth, open: rest === '(' }
  }

  const { context, variation } = nesting.enter(depth)
  if (variation !== (time === null)) {
    throw new SyntaxError(variation ? 'lines inside a variation have time -' : 'time - is only allowed inside a variation')
  }
  const c = new Cursor(tokenize(rest))
  const statement = parseStatement(c, context)

  let cause = null
  const replacements: Id[] = []
  if (c.take('by')) {
    cause = c.cause()
    while (c.take('replacement')) replacements.push(c.id())
  }
  const nags: number[] = []
  while (c.peek()?.t === 'nag') nags.push((c.next() as { v: number }).v)
  let braceComment: string | null = null
  let lineComment: string | null = null
  const t1 = c.peek()
  if (t1?.t === 'brace') {
    braceComment = t1.v
    c.next()
  }
  const t2 = c.peek()
  if (t2?.t === 'comment') {
    lineComment = t2.v
    c.next()
  }
  if (!c.done()) throw new SyntaxError(`unexpected text after the statement: ${describeRest(c)}`)

  nesting.push(depth, statement, variation)
  return {
    kind: 'statement',
    line: n,
    time,
    depth,
    statement,
    cause,
    replacements,
    nags,
    braceComment,
    lineComment,
    variation,
  }
}

function describeRest(c: Cursor): string {
  const t = c.peek()!
  switch (t.t) {
    case 'string':
      return JSON.stringify(t.v)
    case 'word':
      return t.v
    case 'symbols':
      return t.raw
    case 'id':
      return `#${t.v.hypothetical ? 'h' : ''}${t.v.n}`
    default:
      return t.t
  }
}
