import { SyntaxError, type Token } from './tokenizer.js'
import {
  CARD_TYPES,
  RULE_NAMES,
  STEPS,
  type AbilityKind,
  type CardType,
  type CardTypes,
  type Cause,
  type Choice,
  type Entering,
  type Id,
  type MoveAction,
  type ObjRef,
  type Position,
  type Pt,
  type PtValue,
  type RuleName,
  type Statement,
  type Step,
  type Target,
  type Zone,
} from './types.js'

/** What a line's parent allows at that nesting level (section 5.2). */
export type Context = 'top' | 'deck' | 'pregame' | 'turn' | 'keyframe' | 'step' | 'priority' | 'stack'

const ZONES = new Set(['library', 'hand', 'graveyard', 'battlefield', 'stack', 'exile', 'command', 'ante'])
const KINDS = new Set<AbilityKind>(['activated', 'triggered', 'loyalty', 'mana'])
const COLORS = new Set(['white', 'blue', 'black', 'red', 'green'])
const ACTIONS = new Set(['draw', 'discard', 'sacrifice', 'destroy', 'mill'])
const STEP_SET = new Set<string>(STEPS)
const RULE_SET = new Set<string>(RULE_NAMES)
const EXTENSION = /^[A-Z]+_[A-Za-z0-9_]+$/
const MANA_SYMBOL = /^[WUBRGC]$/
const COST_SYMBOL = /^[WUBRGCS]$/
const GENERIC = /^[1-9][0-9]*$/
const SPLIT_SYMBOL = /^([WUBRG]\/[WUBRG]|[2C]\/[WUBRG]|[WUBRG]\/P|[WUBRG]\/[WUBRG]\/P)$/
const TIME = /^(0|[1-9][0-9]*):([0-5][0-9])(?:\.([0-9]{1,3}))?$/

export function parseTime(text: string): number | null {
  const m = TIME.exec(text)
  if (!m) return null
  return Number(m[1]) * 60 + Number(m[2]) + (m[3] ? Number(`0.${m[3]}`) : 0)
}

export class Cursor {
  private i = 0
  constructor(private readonly tokens: Token[]) {}

  peek(): Token | undefined {
    return this.tokens[this.i]
  }
  peekWord(): string | null {
    const t = this.peek()
    return t?.t === 'word' ? t.v : null
  }
  atWord(...words: string[]): boolean {
    const w = this.peekWord()
    return w !== null && words.includes(w)
  }
  done(): boolean {
    return this.i >= this.tokens.length
  }
  next(): Token {
    const t = this.tokens[this.i]
    if (!t) throw new SyntaxError('line ends early')
    this.i += 1
    return t
  }
  /** Consumes the word if it is next; returns whether it was. */
  take(word: string): boolean {
    if (this.atWord(word)) {
      this.i += 1
      return true
    }
    return false
  }
  word(): string {
    const t = this.next()
    if (t.t !== 'word') throw new SyntaxError(`expected a keyword, got ${describe(t)}`)
    return t.v
  }
  keyword(word: string): void {
    const t = this.next()
    if (t.t !== 'word' || t.v !== word) throw new SyntaxError(`expected ${word}, got ${describe(t)}`)
  }
  string(): string {
    const t = this.next()
    if (t.t !== 'string') throw new SyntaxError(`expected a string, got ${describe(t)}`)
    return t.v
  }
  optionalString(): string | null {
    return this.peek()?.t === 'string' ? this.string() : null
  }
  /** A card name, or null for `?`: a name the writer could not determine (section 6.4, decision 0048). */
  cardName(): string | null {
    if (this.take('?')) return null
    return this.string()
  }
  /** A string or `?`: an ability's or effect's text the source did not report (sections 6.5, 9.10). */
  text(): string | null {
    if (this.take('?')) return null
    return this.string()
  }
  int(): number {
    const t = this.next()
    if (t.t !== 'int' || t.negative) throw new SyntaxError(`expected an integer, got ${describe(t)}`)
    return t.v
  }
  sint(): number {
    const t = this.next()
    if (t.t !== 'int') throw new SyntaxError(`expected an integer, got ${describe(t)}`)
    return t.v
  }
  id(): Id {
    const t = this.next()
    if (t.t !== 'id') throw new SyntaxError(`expected an instance ID, got ${describe(t)}`)
    return t.v
  }
  objref(): ObjRef {
    return { id: this.id(), name: this.optionalString() }
  }
  zone(): Zone {
    const w = this.word()
    if (w === 'outside') return { zone: 'outside', label: this.string() }
    if (!ZONES.has(w)) throw new SyntaxError(`unknown zone ${w}`)
    return { zone: w as Exclude<Zone, { zone: 'outside' }>['zone'] }
  }
  pt(): Pt {
    const t = this.next()
    if (t.t !== 'pt') throw new SyntaxError(`expected power/toughness, got ${describe(t)}`)
    return { power: t.power, toughness: t.toughness }
  }
  ptValue(): PtValue {
    if (this.take('none')) return 'none'
    if (this.take('?')) return '?'
    return this.pt()
  }
  symbols(): string[] {
    const t = this.next()
    if (t.t !== 'symbols') throw new SyntaxError(`expected mana symbols, got ${describe(t)}`)
    return t.v
  }
  mana(): string[] {
    const syms = this.symbols()
    for (const s of syms) {
      if (!MANA_SYMBOL.test(s)) throw new SyntaxError(`{${s}} is not a type of mana`)
    }
    return syms
  }
  cost(): string[] {
    const syms = this.symbols()
    const first = syms[0]!
    if (first === '0' && syms.length === 1) return syms
    const rest = GENERIC.test(first) ? syms.slice(1) : syms
    for (const s of rest) {
      if (!COST_SYMBOL.test(s)) throw new SyntaxError(`{${s}} is not a cost symbol`)
    }
    return syms
  }
  /** `PLAYER|OBJREF`, the target shape shared by `target`, `damage`, `attack`, and `attacking`. */
  target(): Target {
    if (this.peek()?.t === 'string') return { kind: 'player', player: this.string() }
    return { kind: 'object', object: this.objref() }
  }
  time(): number {
    const w = this.word()
    const t = parseTime(w)
    if (t === null) throw new SyntaxError(`bad time ${w}`)
    return t
  }
  position(): Position | null {
    if (this.atWord('top', 'bottom')) {
      const end = this.word() as 'top' | 'bottom'
      return { end, n: this.int() }
    }
    return null
  }
  /** `TYPES` (section 7.7): `?`, or card types in the order CR 205.2a lists them, each once. */
  cardTypes(): CardTypes {
    if (this.take('?')) return '?'
    const types: CardType[] = []
    while (this.atWord(...CARD_TYPES)) {
      const type = this.word() as CardType
      const last = types.at(-1)
      if (last !== undefined && CARD_TYPES.indexOf(type) <= CARD_TYPES.indexOf(last)) {
        throw new SyntaxError(`card type ${type} is out of order or repeated`)
      }
      types.push(type)
    }
    if (types.length === 0) throw new SyntaxError('expected a card type')
    return types
  }
  entering(): Entering {
    const face = this.take('face') ? this.string() : null
    const tapped = this.take('tapped')
    const attacking = this.take('attacking') ? this.target() : null
    const types = this.take('types') ? this.cardTypes() : null
    const pt = this.take('pt') ? this.pt() : null
    return { face, tapped, attacking, types, pt }
  }
  printing(): { set: string; number: string } | null {
    if (!this.take('print')) return null
    return { set: this.string(), number: this.string() }
  }
  sources(): string[] {
    const out: string[] = []
    while (this.take('src')) {
      const w = this.word()
      if (!/^[a-z]+:[A-Za-z0-9._-]+$/.test(w)) throw new SyntaxError(`bad source ID ${w}`)
      out.push(w)
    }
    return out
  }
  choice(): Choice {
    const kind = this.word()
    switch (kind) {
      case 'mode':
      case 'cost':
      case 'option':
      case 'type':
        return { kind, text: this.string() }
      case 'x':
      case 'number':
        return { kind, value: this.int() }
      case 'symbol': {
        const [symbol, ...extra] = this.symbols()
        if (extra.length || !SPLIT_SYMBOL.test(symbol!)) throw new SyntaxError(`bad hybrid or Phyrexian symbol`)
        if (this.take('life')) return { kind, symbol: symbol!, payment: 'life' }
        const [payment, ...more] = this.symbols()
        if (more.length || !/^[WUBRGC2]$/.test(payment!)) throw new SyntaxError(`bad symbol payment`)
        return { kind, symbol: symbol!, payment: payment! }
      }
      case 'color': {
        const color = this.word()
        if (!COLORS.has(color)) throw new SyntaxError(`unknown color ${color}`)
        return { kind, color: color as Extract<Choice, { kind: 'color' }>['color'] }
      }
      case 'object':
        return { kind, object: this.objref() }
      case 'player':
        return { kind, player: this.string() }
      case 'name':
        return { kind, name: this.string() }
      case 'hidden':
        return { kind }
      default:
        throw new SyntaxError(`unknown choice ${kind}`)
    }
  }
  /** `CHOICE` in a stack entry: a `choose` value or `target ...` (section 9.9). */
  snapChoices(): Choice[] {
    const out: Choice[] = []
    while (!this.done()) {
      if (this.take('target')) {
        const target = this.target()
        out.push({ kind: 'target', target, amount: this.take('amount') ? this.int() : null })
      } else out.push(this.choice())
    }
    return out
  }
  cause(): Cause {
    const t = this.next()
    if (t.t === 'id') return { by: 'object', id: t.v }
    if (t.t === 'string') return { by: 'player', player: t.v }
    if (t.t !== 'word') throw new SyntaxError(`bad cause ${describe(t)}`)
    if (t.v === '?') return { by: 'unknown' }
    if (t.v.startsWith('cost:#')) {
      const m = /^cost:#([1-9][0-9]*)$/.exec(t.v)
      if (!m) throw new SyntaxError(`bad cost cause ${t.v}`)
      return { by: 'cost', id: { n: Number(m[1]), hypothetical: false } }
    }
    if (t.v.startsWith('rule:')) {
      const rule = t.v.slice(5)
      if (!RULE_SET.has(rule)) throw new SyntaxError(`unknown rule ${rule}`)
      return { by: 'rule', rule: rule as RuleName }
    }
    if (t.v === 'source:clock' || t.v === 'source:disconnect') {
      return { by: 'source', source: t.v.slice(7) as 'clock' | 'disconnect' }
    }
    throw new SyntaxError(`bad cause ${t.v}`)
  }
}

function describe(t: Token): string {
  switch (t.t) {
    case 'string':
      return JSON.stringify(t.v)
    case 'id':
      return t.v.hypothetical ? `#h${t.v.n}` : `#${t.v.n}`
    case 'int':
      return String(t.v)
    case 'pt':
      return `${t.power}/${t.toughness}`
    case 'symbols':
      return t.raw
    case 'nag':
      return `$${t.v}`
    case 'brace':
      return 'a brace comment'
    case 'comment':
      return 'a line comment'
    case 'word':
      return t.v
  }
}

/** Parses the statement at the cursor in the given context. Leaves the cursor at the cause or annotations. */
export function parseStatement(c: Cursor, context: Context): Statement {
  const t = c.peek()
  if (!t) throw new SyntaxError('empty statement')
  if (t.t === 'word' && EXTENSION.test(t.v)) return extension(c)
  switch (context) {
    case 'top':
      return top(c)
    case 'deck':
      return deckEntry(c)
    case 'pregame':
      return c.atWord('keyframe') ? structural(c) : event(c)
    case 'turn':
      return c.atWord('keyframe') ? structural(c) : stepLine(c)
    case 'keyframe':
      return keyframeEntry(c)
    case 'step':
      return event(c)
    case 'priority':
      if (c.take('stack')) return { type: 'stack' }
      throw new SyntaxError('only a stack line may follow a priority line at this level')
    case 'stack':
      return stackEntry(c)
  }
}

function extension(c: Cursor): Statement {
  const name = c.word()
  const args: Extract<Statement, { type: 'extension' }>['args'] = []
  while (!c.done()) {
    const t = c.peek()!
    if (t.t === 'string') args.push(c.string())
    else if (t.t === 'int' && !t.negative) args.push(c.int())
    else if (t.t === 'word' && /^[a-z-]+$/.test(t.v)) args.push({ keyword: c.word() })
    else break
  }
  return { type: 'extension', name, args }
}

function top(c: Cursor): Statement {
  const w = c.peekWord()
  if (w === 'deck') {
    c.word()
    const player = c.string()
    const name = c.take('name') ? c.string() : null
    return { type: 'deck', player, name, unknown: c.take('unknown') }
  }
  if (w === 'pregame' || w === 'turn') return structural(c)
  throw new SyntaxError(`expected deck, pregame, or turn at the top level, got ${w ?? describe(c.peek()!)}`)
}

function structural(c: Cursor): Statement {
  const w = c.word()
  switch (w) {
    case 'pregame':
      return { type: 'pregame' }
    case 'keyframe':
      return { type: 'keyframe' }
    case 'turn':
      return { type: 'turn', number: c.int(), player: c.string() }
  }
  throw new SyntaxError(`unexpected ${w}`)
}

function stepLine(c: Cursor): Statement {
  const w = c.word()
  if (!STEP_SET.has(w)) throw new SyntaxError(`expected a step or main phase under turn, got ${w}`)
  return { type: 'step', step: w as Step }
}

function deckEntry(c: Cursor): Statement {
  const board = c.word()
  if (board !== 'main' && board !== 'side') throw new SyntaxError(`expected main or side, got ${board}`)
  const count = c.int()
  const name = c.cardName()
  if (name !== null) return { type: 'deck-entry', board, count, name, src: [] }
  // Section 10.5: an entry named `?` carries exactly one source ID.
  const src = c.sources()
  if (src.length !== 1) throw new SyntaxError('a decklist entry named ? needs exactly one source ID')
  return { type: 'deck-entry', board, count, name, src }
}

function event(c: Cursor): Statement {
  const t = c.peek()!
  if (t.t === 'id') return objectStatement(c)
  if (t.t === 'string') return playerStatement(c)
  const w = c.word()
  switch (w) {
    case 'new':
      return newStatement(c)
    case 'incomplete':
      return { type: 'incomplete', reason: c.string() }
    case 'game-draw':
      return { type: 'game-draw' }
    case 'day':
      return { type: 'day' }
    case 'night':
      return { type: 'night' }
  }
  if (STEP_SET.has(w) || w === 'keyframe' || w === 'stack' || w === 'turn' || w === 'pregame' || w === 'deck') {
    throw new SyntaxError(`${w} is not allowed at this nesting level`)
  }
  throw new SyntaxError(`unknown keyword ${w}`)
}

function newStatement(c: Cursor): Statement {
  const what = c.word()
  switch (what) {
    case 'card': {
      const id = c.id()
      const name = c.cardName()
      c.keyword('owner')
      const owner = c.string()
      c.keyword('in')
      const zone = c.zone()
      const position = c.position()
      return { type: 'new-card', id, name, owner, zone, position, entering: c.entering(), print: c.printing(), src: c.sources() }
    }
    case 'hidden': {
      const id = c.id()
      c.keyword('owner')
      const owner = c.string()
      c.keyword('in')
      return { type: 'new-hidden', id, owner, zone: c.zone(), position: c.position() }
    }
    case 'token': {
      const id = c.id()
      const name = c.cardName()
      c.keyword('owner')
      const owner = c.string()
      c.keyword('in')
      const zone = c.zone()
      return { type: 'new-token', id, name, owner, zone, entering: c.entering(), src: c.sources() }
    }
    case 'copy': {
      const id = c.id()
      c.keyword('of')
      const ofId = c.id()
      const strings: string[] = []
      while (c.peek()?.t === 'string') strings.push(c.string())
      if (c.take('controller')) {
        if (strings.length) throw new SyntaxError('a copy of an ability has no name')
        const controller = c.string()
        c.keyword('in')
        c.keyword('stack')
        return { type: 'new-ability-copy', id, of: ofId, controller }
      }
      // The copy's own name is last: a string, or `?` after the copied object's optional name.
      const unknownName = strings.length <= 1 && c.take('?')
      const nameCount = unknownName ? 0 : 1
      if (strings.length < nameCount || strings.length > nameCount + 1) throw new SyntaxError('expected the copied object and the copy name')
      const name = unknownName ? null : strings[strings.length - 1]!
      const of: ObjRef = strings.length > nameCount ? { id: ofId, name: strings[0]! } : { id: ofId, name: null }
      c.keyword('owner')
      const owner = c.string()
      c.keyword('in')
      return { type: 'new-copy', id, of, name, owner, zone: c.zone(), entering: c.entering() }
    }
    case 'ability': {
      const id = c.id()
      const kind = c.word()
      if (!KINDS.has(kind as AbilityKind)) throw new SyntaxError(`unknown ability kind ${kind}`)
      c.keyword('of')
      const of = c.objref()
      const text = c.text()
      c.keyword('controller')
      const controller = c.string()
      let inStack = false
      if (c.take('in')) {
        c.keyword('stack')
        inStack = true
      }
      const from = c.take('from') ? c.id() : null
      return { type: 'new-ability', id, kind: kind as AbilityKind, of, text, controller, inStack, from }
    }
    case 'effect': {
      const id = c.id()
      c.keyword('of')
      const of = c.objref()
      const text = c.text()
      c.keyword('controller')
      return { type: 'new-effect', id, of, text, controller: c.string() }
    }
    case 'emblem': {
      const id = c.id()
      const text = c.text()
      c.keyword('owner')
      return { type: 'new-emblem', id, text, owner: c.string() }
    }
  }
  throw new SyntaxError(`unknown object kind ${what} after new`)
}

function objectStatement(c: Cursor): Statement {
  const object = c.objref()
  const verb = c.word()
  switch (verb) {
    case 'play': {
      c.keyword('from')
      const from = c.zone()
      const face = c.take('face') ? c.string() : null
      const tapped = c.take('tapped')
      const types = c.take('types') ? c.cardTypes() : null
      const pt = c.take('pt') ? c.pt() : null
      return { type: 'play', object, from, face, tapped, types, pt }
    }
    case 'cast': {
      c.keyword('from')
      const from = c.zone()
      const face = c.take('face') ? c.string() : null
      const fused = face === null && c.take('fused')
      return { type: 'cast', object, from, face, fused }
    }
    case 'move': {
      const from = c.zone()
      c.keyword('->')
      const to = c.zone()
      let as: MoveAction | null = null
      if (c.take('as')) {
        const a = c.word()
        if (!ACTIONS.has(a)) throw new SyntaxError(`unknown action ${a}`)
        as = a as MoveAction
      }
      const position = c.atWord('top', 'bottom') ? (c.word() as 'top' | 'bottom') : null
      // Section 7.1: `top N` (N of 2 or more; `top` alone is the top card) and `bottom ?` (random order).
      let nth: number | null = null
      let random = false
      if (position === 'top' && c.peek()?.t === 'int') {
        nth = c.int()
        if (nth < 2) throw new SyntaxError('a move to the top of a library is written top, not top 1')
      } else if (position === 'bottom' && c.take('?')) {
        random = true
      }
      const entering = c.entering()
      const w = c.take('with') ? c.id() : null
      return { type: 'move', object, from, to, as, position, nth, random, entering, with: w }
    }
    case 'choose':
      return { type: 'choose', object, choice: c.choice() }
    case 'target': {
      const target = c.target()
      return { type: 'target', object, target, amount: c.take('amount') ? c.int() : null }
    }
    case 'tap':
    case 'untap':
    case 'unattach':
    case 'reveal':
    case 'untracked':
    case 'cease':
    case 'transform':
    case 'goad':
    case 'resolve':
    case 'fizzle':
    case 'countered':
      return { type: verb, object }
    case 'end-goad':
      return { type: verb, object, player: c.string() }
    case 'look':
      return { type: verb, object, player: c.string() }
    case 'become-copy':
      return { type: verb, object, of: c.objref() }
    case 'special': {
      const action = c.word()
      if (action !== 'companion' && action !== 'suspend' && action !== 'plot') {
        throw new SyntaxError(`unknown special action ${action}`)
      }
      return { type: verb, object, action }
    }
    case 'attach':
      return { type: verb, object, to: c.objref() }
    case 'control':
      return { type: verb, object, player: c.string() }
    case 'counters': {
      const kind = c.string()
      const old = c.int()
      c.keyword('->')
      return { type: verb, object, kind, old, new: c.int() }
    }
    case 'pt': {
      const old = c.ptValue()
      c.keyword('->')
      return { type: verb, object, old, new: c.ptValue() }
    }
    case 'types': {
      const old = c.cardTypes()
      c.keyword('->')
      return { type: verb, object, old, new: c.cardTypes() }
    }
    case 'cost': {
      const old = c.cost()
      c.keyword('->')
      return { type: verb, object, old, new: c.cost() }
    }
    case 'damage': {
      const to = c.target()
      const amount = c.int()
      return { type: verb, object, to, amount, prevented: c.take('prevented') }
    }
    case 'attack':
      return { type: verb, object, target: c.target() }
    case 'block':
      return { type: verb, object, attacker: c.objref() }
    case 'identify': {
      const name = c.cardName()
      return { type: verb, object, name, print: c.printing(), src: c.sources() }
    }
  }
  throw new SyntaxError(`unknown keyword ${verb}`)
}

function playerStatement(c: Cursor): Statement {
  const player = c.string()
  const verb = c.word()
  switch (verb) {
    case 'roll': {
      let sides: number | null = null
      const w = c.peekWord()
      if (w && /^d[1-9][0-9]*$/.test(w)) {
        c.word()
        sides = Number(w.slice(1))
      }
      return { type: 'roll', player, sides, result: c.int() }
    }
    case 'choose':
      if (c.take('first')) return { type: 'choose-first', player, first: c.string() }
      return { type: 'player-choose', player, choice: c.choice() }
    case 'mulligan':
    case 'keep':
    case 'pass':
    case 'concede':
    case 'lose':
    case 'win':
    case 'shuffle':
      return { type: verb, player }
    case 'search': {
      const zone = c.zone()
      return { type: verb, player, zone, of: c.optionalString() }
    }
    case 'priority': {
      if (!c.take('clock')) return { type: verb, player, clock: null }
      const clock = [
        { player: c.string(), time: c.time() },
        { player: c.string(), time: c.time() },
      ]
      return { type: verb, player, clock }
    }
    case 'add': {
      const mana = c.mana()
      return { type: verb, player, mana, restriction: c.take('restriction') ? (c.take('?') ? { unknown: true } : { text: c.string() }) : null }
    }
    case 'pay':
    case 'lose-mana':
      return { type: verb, player, mana: c.mana() }
    case 'life': {
      const old = c.sint()
      c.keyword('->')
      return { type: verb, player, old, new: c.sint() }
    }
    case 'poison': {
      const old = c.int()
      c.keyword('->')
      return { type: verb, player, old, new: c.int() }
    }
    case 'counters': {
      const kind = c.string()
      const old = c.int()
      c.keyword('->')
      return { type: 'player-counters', player, kind, old, new: c.int() }
    }
  }
  throw new SyntaxError(`unknown keyword ${verb}`)
}

function counterList(c: Cursor): { kind: string; count: number }[] {
  const out: { kind: string; count: number }[] = []
  while (c.take('counters')) out.push({ kind: c.string(), count: c.int() })
  return out
}

function keyframeEntry(c: Cursor): Statement {
  const t = c.peek()!
  if (t.t === 'word') {
    const w = c.word()
    switch (w) {
      case 'player': {
        const player = c.string()
        c.keyword('life')
        const life = c.sint()
        c.keyword('poison')
        const poison = c.int()
        c.keyword('library')
        const library = c.int()
        const counters = counterList(c)
        const clock = c.take('clock') ? c.time() : null
        return { type: 'keyframe-player', player, life, poison, library, counters, clock }
      }
      case 'round-clock':
        return { type: 'round-clock', time: c.time() }
      case 'day':
        return { type: 'day' }
      case 'night':
        return { type: 'night' }
    }
    throw new SyntaxError(`unknown keyframe entry ${w}`)
  }
  const id = c.id()
  if (c.take('effect')) {
    c.keyword('of')
    const of = c.objref()
    const text = c.text()
    c.keyword('controller')
    return { type: 'keyframe-effect', id, of, text, controller: c.string() }
  }
  const object: ObjRef = { id, name: c.optionalString() }
  const zone = c.zone()
  const player = c.optionalString()
  const face = c.take('face') ? c.string() : null
  const types = c.take('types') ? c.cardTypes() : null
  const pt = c.take('pt') ? c.ptValue() : null
  const tapped = c.take('tapped')
  const attached = c.take('attached') ? c.id() : null
  const w = c.take('with') ? c.id() : null
  const adventure = c.take('adventure') ? c.string() : null
  const companion = c.take('companion')
  const goaded: string[] = []
  while (c.take('goaded')) goaded.push(c.string())
  const counters = counterList(c)
  const position = c.position()
  const emblem = c.take('emblem') ? c.string() : null
  return {
    type: 'keyframe-object',
    object,
    zone,
    player,
    face,
    types,
    pt,
    tapped,
    attached,
    with: w,
    adventure,
    companion,
    goaded,
    counters,
    position,
    emblem,
  }
}

function stackEntry(c: Cursor): Statement {
  const id = c.id()
  const kindWord = c.peekWord()
  if (kindWord && KINDS.has(kindWord as AbilityKind)) {
    const kind = c.word() as AbilityKind
    c.keyword('of')
    const of = c.objref()
    const text = c.text()
    c.keyword('controller')
    const controller = c.string()
    return { type: 'stack-ability', id, kind, of, text, controller, choices: c.snapChoices() }
  }
  const name = c.optionalString()
  const face = c.take('face') ? c.string() : null
  const fused = face === null && c.take('fused')
  c.keyword('controller')
  const controller = c.string()
  return { type: 'stack-spell', object: { id, name }, face, fused, controller, choices: c.snapChoices() }
}
