import { compareKeyframe, compareStack } from './compare.js'
import { parse } from './parser.js'
import { apply, getObject, initialState, type GameState, type ManaType } from './replay.js'
import {
  formatZone,
  idKey,
  sameId,
  sameTypes,
  statementFamilies,
  sameZone,
  tagValue,
  type CardTypes,
  type Game,
  type StatementFamily,
  type Id,
  type ParseResult,
  type Problem,
  type PtValue,
  type Result,
  type Statement,
  type StatementLine,
} from './types.js'

/** Runs every check in section 14.1. Parse problems are check 1. */
export function validate(input: string | ParseResult): Problem[] {
  const result = typeof input === 'string' ? parse(input) : input
  const problems = [...result.problems]
  const games = result.file.games
  for (const [i, game] of games.entries()) {
    // A game with syntax problems stops at check 1: a dropped line would only cascade into false replay failures.
    const first = game.tags[0]?.line ?? game.body[0]?.line ?? game.resultLine
    const last = games[i + 1]?.tags[0]?.line ?? Infinity
    if (result.problems.some((p) => p.line >= first && p.line < last)) continue
    problems.push(...validateGame(game))
  }
  return problems.sort((a, b) => a.line - b.line || a.check - b.check)
}

type CauseRule = 'required' | 'optional' | 'none' | 'chooser' | 'any'

/** Section 8: which statements take a cause. */
const CAUSE_RULE: Record<Statement['type'], CauseRule> = {
  deck: 'none',
  'deck-entry': 'none',
  pregame: 'none',
  turn: 'none',
  step: 'none',
  keyframe: 'none',
  stack: 'none',
  incomplete: 'none',
  'game-draw': 'required',
  day: 'required',
  night: 'required',
  extension: 'any',
  'new-card': 'none',
  'new-hidden': 'none',
  'new-token': 'required',
  'new-copy': 'required',
  'new-ability-copy': 'required',
  'new-ability': 'none',
  'new-effect': 'required',
  'new-emblem': 'required',
  play: 'none',
  cast: 'optional',
  move: 'required',
  choose: 'chooser',
  target: 'none',
  tap: 'required',
  untap: 'required',
  attach: 'required',
  unattach: 'required',
  control: 'required',
  counters: 'required',
  pt: 'required',
  types: 'required',
  cost: 'required',
  damage: 'required',
  attack: 'none',
  block: 'none',
  reveal: 'required',
  look: 'required',
  identify: 'none',
  untracked: 'none',
  transform: 'required',
  'become-copy': 'required',
  special: 'none',
  goad: 'required',
  'end-goad': 'required',
  cease: 'required',
  resolve: 'none',
  fizzle: 'optional',
  countered: 'required',
  roll: 'none',
  'choose-first': 'none',
  'player-choose': 'required',
  mulligan: 'none',
  keep: 'none',
  pass: 'none',
  concede: 'none',
  lose: 'required',
  win: 'required',
  shuffle: 'required',
  search: 'required',
  priority: 'none',
  add: 'required',
  pay: 'required',
  'lose-mana': 'required',
  life: 'required',
  poison: 'required',
  'player-counters': 'required',
  'keyframe-player': 'none',
  'round-clock': 'none',
  'keyframe-object': 'none',
  'keyframe-effect': 'none',
  'stack-spell': 'none',
  'stack-ability': 'none',
}

const GAME_ENDERS = new Set<Statement['type']>(['concede', 'lose', 'win', 'game-draw'])

interface Ref {
  id: Id
  /** The name written next to the ID; undefined when the grammar has a bare ID there (`with ID`, `by ID`). */
  name: string | null | undefined
}

const isId = (v: unknown): v is Id =>
  typeof v === 'object' && v !== null && typeof (v as Id).n === 'number' && typeof (v as Id).hypothetical === 'boolean'

/** Every ID a statement refers to, other than the one a `new` line introduces. */
function refsOf(statement: Statement): Ref[] {
  const out: Ref[] = []
  const walk = (v: unknown) => {
    if (Array.isArray(v)) {
      v.forEach(walk)
      return
    }
    if (typeof v !== 'object' || v === null) return
    if (isId(v)) {
      out.push({ id: v, name: undefined })
      return
    }
    const o = v as Record<string, unknown>
    if ('id' in o && 'name' in o && isId(o['id']) && Object.keys(o).length === 2) {
      out.push({ id: o['id'], name: o['name'] as string | null })
      return
    }
    for (const [k, val] of Object.entries(o)) {
      if (k === 'id' && statement.type.startsWith('new-')) continue
      walk(val)
    }
  }
  walk(statement)
  return out
}

const samePt = (a: PtValue | null, b: PtValue): boolean => {
  const x = a ?? 'none'
  if (typeof x === 'string' || typeof b === 'string') return x === b
  return x.power === b.power && x.toughness === b.toughness
}

function validateGame(game: Game): Problem[] {
  const problems: Problem[] = []
  const report = (line: number, check: number, message: string) => problems.push({ line, check, message })
  const player1 = tagValue(game, 'Player1')
  const player2 = tagValue(game, 'Player2')
  const players = new Set([player1, player2])
  const resultTag = tagValue(game, 'Result')
  // Section 4.7: Omits lists the families the source never records, Partial those it records in part.
  const familyTag = (name: 'Omits' | 'Partial') => {
    const tag = game.tags.find((t) => t.name === name)
    const families = statementFamilies(tag?.value ?? null)
    if (tag && families === null) report(tag.line, 1, `${name} ${JSON.stringify(tag.value)} is not a list of known families in order`)
    return { tag, families: families ?? new Set<StatementFamily>() }
  }
  const omits = familyTag('Omits')
  const partial = familyTag('Partial')
  for (const family of partial.families) {
    if (omits.families.has(family)) report(partial.tag!.line, 1, `${family} is in both Omits and Partial`)
  }
  /** Families either tag lists: both waive the same checks. */
  const waivedFamilies = new Set<StatementFamily>([...omits.families, ...partial.families])
  const waived = new Set([...(waivedFamilies.has('mana') ? [11] : []), ...(waivedFamilies.has('library') ? [13] : [])])
  const replayReport = (line: number, check: number, message: string) => {
    if (!waived.has(check)) report(line, check, message)
  }
  if (resultTag !== null && resultTag !== game.result) {
    report(game.resultLine, 2, `result line ${game.result} does not equal the Result tag ${resultTag}`)
  }

  const body = game.body
  /** Direct children of the statement line at `i` (one level deeper, until a line at its level or above). */
  const childrenOf = (i: number): StatementLine[] => {
    const parent = body[i] as StatementLine
    const out: StatementLine[] = []
    for (let j = i + 1; j < body.length; j++) {
      const l = body[j]!
      if (l.kind === 'escape') continue
      if (l.depth <= parent.depth) break
      if (l.kind === 'statement' && l.depth === parent.depth + 1) out.push(l)
    }
    return out
  }
  const parentOf = (i: number): StatementLine | null => {
    const me = body[i] as StatementLine
    for (let j = i - 1; j >= 0; j--) {
      const l = body[j]!
      if (l.kind === 'statement' && l.depth < me.depth) return l
    }
    return null
  }

  let state = initialState(game)
  let lastTime = 0
  let lastTurn = 0
  let incompleteSinceTurn = false
  let keyframes = 0
  let topLevel = 0
  let nextId = 1
  let endedAt: { line: number; expected: Result } | null = null
  let lastMainStatement: StatementLine | null = null
  const openVariations: number[] = []

  for (const [i, line] of body.entries()) {
    if (line.kind === 'escape') continue
    if (line.kind === 'variation') {
      if (line.open) openVariations.push(line.line)
      else if (openVariations.pop() === undefined) report(line.line, 17, 'closing bracket without an open variation')
      continue
    }
    if (line.variation) continue
    const s = line.statement
    const before = state
    const suspended = before.incomplete
    lastMainStatement = line

    const refs = refsOf(s)
    const causeId = line.cause && (line.cause.by === 'object' || line.cause.by === 'cost') ? line.cause.id : null
    for (const id of [...refs.map((r) => r.id), ...(causeId ? [causeId] : []), ...line.replacements]) {
      if (id.hypothetical) report(line.line, 17, `hypothetical ID ${idKey(id)} outside a variation`)
    }

    // Check 1: sequence rules the grammar cannot express.
    if (line.time !== null) {
      if (line.time < lastTime) report(line.line, 1, 'time decreases from the previous line')
      lastTime = line.time
    }
    if (line.depth === 0) {
      const expected = topLevel === 0 ? 'deck' : topLevel === 1 ? 'pregame' : 'turn'
      if (s.type !== expected && s.type !== 'extension') report(line.line, 1, `expected ${expected} here, got ${s.type}`)
      if (s.type !== 'extension') topLevel++
    }
    if (s.type === 'turn') {
      if (incompleteSinceTurn ? s.number <= lastTurn : s.number !== lastTurn + 1) {
        report(line.line, 1, `turn ${s.number} follows turn ${lastTurn}`)
      }
      lastTurn = s.number
      incompleteSinceTurn = false
    }
    if (s.type === 'incomplete') incompleteSinceTurn = true
    for (const kind of counterNames(s)) {
      if (kind !== kind.toLowerCase()) report(line.line, 1, `counter name ${JSON.stringify(kind)} has an uppercase letter`)
      if (kind === 'poison') report(line.line, 1, 'poison counters are written with poison, not counters "poison"')
    }
    // Check 18: a permanent put onto the battlefield, or listed there in a keyframe, has its types, and pt only as a creature.
    const permanent = battlefieldEntry(s)
    if (permanent) {
      if (permanent.types === null) report(line.line, 18, 'a permanent on the battlefield needs types')
      else if (permanent.types !== '?' && permanent.types.includes('creature') !== permanent.hasPt) {
        report(line.line, 18, permanent.hasPt ? 'pt is written but the types do not include creature' : 'a creature needs pt')
      }
    }
    if ('player' in s && typeof s.player === 'string' && s.type !== 'keyframe-object' && !players.has(s.player)) {
      report(line.line, 1, `${JSON.stringify(s.player)} is neither Player1 nor Player2`)
    }

    // Check 16: nothing but extensions after the game ends.
    if (endedAt && s.type !== 'extension') report(line.line, 16, 'statement after the line that ended the game')
    if (GAME_ENDERS.has(s.type)) {
      const other = (p: string) => (p === player1 ? player2 : player1)
      const winner =
        s.type === 'game-draw' ? null : s.type === 'win' ? s.player : other((s as { player: string }).player)
      const expected: Result = s.type === 'game-draw' ? '1/2-1/2' : winner === player1 ? '1-0' : '0-1'
      endedAt = { line: line.line, expected }
    }

    // Check 3: IDs introduced once, in order, and used only while live.
    if (s.type.startsWith('new-')) {
      const id = (s as { id: Id }).id
      if (getObject(before, id)) report(line.line, 3, `${idKey(id)} is introduced twice`)
      else if (id.n !== nextId) report(line.line, 3, `${idKey(id)} follows #${nextId - 1}: IDs must increase by one`)
      nextId = id.n + 1
    }
    for (const ref of refs) {
      const o = getObject(before, ref.id)
      if (!o) {
        report(line.line, 3, `${idKey(ref.id)} is used before its new line`)
        continue
      }
      // Keyframe entries and snapshots may repeat IDs that have since ended (a resync, or a target that left).
      if (o.ended && !s.type.startsWith('keyframe-') && !s.type.startsWith('stack-')) {
        report(line.line, 3, `${idKey(ref.id)} has ended`)
      }
      // Check 4: names next to IDs.
      if (ref.name === undefined) continue
      const nameless = o.kind === 'ability' || o.name === null
      if (nameless && ref.name !== null) {
        report(line.line, 4, `${idKey(ref.id)} has ${o.nameUnknown ? 'name ?' : 'no name'} but is written as ${JSON.stringify(ref.name)}`)
      }
      if (!nameless && ref.name !== o.name && !(s.type === 'keyframe-object' && ref.name === null && o.kind === 'emblem')) {
        report(line.line, 4, `${idKey(ref.id)} is ${JSON.stringify(o.name)} but is written as ${JSON.stringify(ref.name)}`)
      }
    }

    // Check 5: causes where the statement allows them, naming known objects.
    const parentType = parentOf(i)?.statement.type
    const rule = (s.type === 'day' || s.type === 'night') && parentType === 'keyframe' ? 'none' : CAUSE_RULE[s.type]
    if (rule === 'required' && !line.cause) report(line.line, 5, `${s.type} needs a cause`)
    if (rule === 'none' && line.cause) report(line.line, 5, `${s.type} takes no cause`)
    if (rule === 'chooser' && line.cause && line.cause.by !== 'player') report(line.line, 5, 'a choice names only the player who chose')
    if (s.type === 'cast' && line.cause && line.cause.by !== 'object') report(line.line, 5, 'cast is caused only by a spell or ability')
    if (causeId && !getObject(before, causeId)) report(line.line, 5, `cause ${idKey(causeId)} is not a known object`)
    if (line.cause?.by === 'player' && !players.has(line.cause.player)) {
      report(line.line, 5, `${JSON.stringify(line.cause.player)} is neither Player1 nor Player2`)
    }

    // Check 15: identify only a hidden object, once.
    if (s.type === 'identify') {
      const o = getObject(before, s.object.id)
      if (o && (o.kind !== 'hidden' || o.name !== null || o.ownNameUnknown)) report(line.line, 15, `${idKey(o.id)} already has ${o.ownNameUnknown ? 'name ?' : 'a name'}`)
    }

    // Check 4: an object whose name is `?` has no known faces (section 6.4).
    const face = faceOf(s)
    if (face !== null) {
      const unknown = face.name === undefined ? getObject(before, face.id)?.nameUnknown === true : face.name === null
      if (unknown) report(line.line, 4, `${idKey(face.id)} has name ? but is written with face ${JSON.stringify(face.face)}`)
    }

    if (!suspended) replayChecks(line, before, childrenOf(i), keyframes, replayReport, waivedFamilies)
    if (s.type === 'keyframe') keyframes++
    state = apply(state, line)
  }

  for (const open of openVariations) report(open, 17, 'variation is not closed')
  if (endedAt) {
    if (resultTag !== null && resultTag !== endedAt.expected) {
      const tag = game.tags.find((t) => t.name === 'Result')!
      report(tag.line, 16, `the game ended with result ${endedAt.expected} but the Result tag says ${resultTag}`)
    }
  } else if (lastMainStatement && lastMainStatement.statement.type !== 'incomplete') {
    report(game.resultLine, 16, 'a game with no ending line must end with an incomplete line')
  }
  return problems
}

/** The checks marked † in section 14.1, against the state before the line. */
function replayChecks(
  line: StatementLine,
  state: GameState,
  children: StatementLine[],
  keyframesSoFar: number,
  report: (line: number, check: number, message: string) => void,
  waivedFamilies: ReadonlySet<StatementFamily>,
): void {
  const s = line.statement
  const n = line.line
  switch (s.type) {
    case 'stack':
      for (const m of compareStack(state, children.map((c) => c.statement))) report(n, 6, m)
      return
    case 'keyframe':
      // The pregame keyframe sets the starting state (section 14.5).
      if (keyframesSoFar === 0) return
      for (const m of compareKeyframe(state, children.map((c) => c.statement), waivedFamilies)) report(n, 7, m)
      return
    case 'priority': {
      const snapshot = children.find((c) => c.statement.type === 'stack')
      if (state.stack.length > 0 && !snapshot) report(n, 14, 'priority with objects on the stack has no stack line')
      if (state.stack.length === 0 && snapshot) report(snapshot.line, 14, 'stack line while the stack is empty')
      return
    }
    case 'resolve':
    case 'fizzle': {
      const top = state.stack[0]
      if (!top || !sameId(top, s.object.id)) report(n, 8, `${idKey(s.object.id)} is not on top of the stack`)
      return
    }
    case 'countered':
      if (!state.stack.some((id) => sameId(id, s.object.id))) report(n, 8, `${idKey(s.object.id)} is not on the stack`)
      return
    case 'play':
    case 'cast':
    case 'move': {
      const o = getObject(state, s.object.id)
      if (!o) return
      if (!o.zone || !sameZone(o.zone, s.from)) {
        report(n, 9, `${idKey(o.id)} is in ${o.zone ? formatZone(o.zone) : 'no zone'}, not ${formatZone(s.from)}`)
      }
      if (s.type === 'move' && (s.as === 'draw' || s.as === 'mill') && o.zone?.zone === 'library') {
        const top = Object.values(state.objects).find(
          (x) => !x.ended && x.zone?.zone === 'library' && x.owner === o.owner && x.position?.end === 'top' && x.position.n === 1,
        )
        if (top && !sameId(top.id, o.id)) report(n, 13, `${idKey(top.id)} is the top card of the library, not ${idKey(o.id)}`)
      }
      // Check 19: `top N` needs N other cards in the library (CR 401.7); with fewer the card goes on the bottom.
      if (s.type === 'move' && s.to.zone === 'library' && s.nth !== null) {
        const size = state.players[o.owner]?.librarySize ?? 0
        const others = o.zone?.zone === 'library' ? size - 1 : size
        if (others < s.nth) report(n, 19, `top ${s.nth} into a library of ${others} other cards: it goes on the bottom (CR 401.7)`)
      }
      return
    }
    case 'life':
    case 'poison': {
      const p = state.players[s.player]
      if (p && p[s.type] !== s.old) report(n, 10, `${s.player} ${s.type} is ${p[s.type]}, not ${s.old}`)
      return
    }
    case 'player-counters': {
      const p = state.players[s.player]
      const current = p?.counters[s.kind] ?? 0
      if (p && current !== s.old) report(n, 10, `${s.player} has ${current} ${s.kind} counters, not ${s.old}`)
      return
    }
    case 'counters': {
      const o = getObject(state, s.object.id)
      const current = o?.counters[s.kind] ?? 0
      if (o && current !== s.old) report(n, 10, `${idKey(o.id)} has ${current} ${s.kind} counters, not ${s.old}`)
      return
    }
    case 'pt': {
      const o = getObject(state, s.object.id)
      if (o && s.old !== '?' && !samePt(o.pt, s.old)) report(n, 10, `${idKey(o.id)} power and toughness differ from OLD`)
      return
    }
    case 'types': {
      const o = getObject(state, s.object.id)
      if (o && s.old !== '?' && o.types !== '?' && !sameTypes(o.types, s.old)) report(n, 10, `${idKey(o.id)} card types differ from OLD`)
      return
    }
    case 'cost': {
      const o = getObject(state, s.object.id)
      if (o && o.cost && o.cost.join('') !== s.old.join('')) {
        report(n, 10, `${idKey(o.id)} cost OLD {${s.old.join('}{')}} is not the previous NEW {${o.cost.join('}{')}}`)
      }
      return
    }
    case 'pay':
    case 'lose-mana': {
      const p = state.players[s.player]
      if (!p) return
      const needed: Partial<Record<ManaType, number>> = {}
      for (const m of s.mana) needed[m as ManaType] = (needed[m as ManaType] ?? 0) + 1
      for (const [m, count] of Object.entries(needed)) {
        if (p.pool[m as ManaType] < count) report(n, 11, `${s.player} has no {${m}} to spend`)
      }
      return
    }
    case 'step':
      for (const [player, p] of Object.entries(state.players)) {
        if (Object.values(p.pool).some((x) => x > 0)) report(n, 11, `${player}'s mana pool is not empty at ${s.step}`)
      }
      return
    case 'tap':
    case 'untap': {
      const o = getObject(state, s.object.id)
      if (o && o.tapped === (s.type === 'tap')) report(n, 12, `${idKey(o.id)} is already ${o.tapped ? 'tapped' : 'untapped'}`)
      return
    }
    default:
      return
  }
}

/** The types and whether pt is written, for a line that puts a permanent onto the battlefield or lists one there (section 7.7). */
function battlefieldEntry(s: Statement): { types: CardTypes | null; hasPt: boolean } | null {
  switch (s.type) {
    case 'play':
      return { types: s.types, hasPt: s.pt !== null }
    case 'move':
      return s.to.zone === 'battlefield' ? { types: s.entering.types, hasPt: s.entering.pt !== null } : null
    case 'new-card':
    case 'new-copy':
    case 'new-token':
      return s.zone.zone === 'battlefield' ? { types: s.entering.types, hasPt: s.entering.pt !== null } : null
    case 'keyframe-object':
      return s.zone.zone === 'battlefield' ? { types: s.types, hasPt: s.pt !== null } : null
    default:
      return null
  }
}

/**
 * The face a line writes for an object: its ID, the face, and for a `new` line the name it introduces
 * (null for `?`). `name` is undefined when the object already exists.
 */
function faceOf(s: Statement): { id: Id; face: string; name?: string | null } | null {
  switch (s.type) {
    case 'play':
    case 'cast':
    case 'stack-spell':
    case 'keyframe-object':
      return s.face === null ? null : { id: s.object.id, face: s.face }
    case 'move':
      return s.entering.face === null ? null : { id: s.object.id, face: s.entering.face }
    case 'new-card':
    case 'new-token':
    case 'new-copy':
      return s.entering.face === null ? null : { id: s.id, face: s.entering.face, name: s.name }
    default:
      return null
  }
}

/** The counter names a line writes (section 9.5). */
function counterNames(s: Statement): string[] {
  switch (s.type) {
    case 'counters':
    case 'player-counters':
      return [s.kind]
    case 'keyframe-player':
    case 'keyframe-object':
      return s.counters.map((c) => c.kind)
    default:
      return []
  }
}
