import {
  idKey,
  sameId,
  tagValue,
  type AbilityKind,
  type BodyLine,
  type CardTypes,
  type Choice,
  type Entering,
  type Game,
  type Id,
  type LibraryEnd,
  type Player,
  type Position,
  type PtValue,
  type Statement,
  type StatementLine,
  type Zone,
} from './types.js'

export type ManaType = 'W' | 'U' | 'B' | 'R' | 'G' | 'C'
export type ManaPool = Readonly<Record<ManaType, number>>
export const EMPTY_POOL: ManaPool = { W: 0, U: 0, B: 0, R: 0, G: 0, C: 0 }

export interface PlayerState {
  life: number
  poison: number
  counters: Readonly<Record<string, number>>
  librarySize: number
  pool: ManaPool
}

export type ObjectKind = 'card' | 'hidden' | 'token' | 'copy' | 'ability' | 'effect' | 'emblem'

export interface ObjectState {
  id: Id
  kind: ObjectKind
  /** Current name: the card's own name, or the copied name after `become-copy`. Null for hidden cards, abilities, and names written `?`. */
  name: string | null
  ownName: string | null
  /** The current name is `?`: the card is known, but the writer could not determine its name (section 6.4, decision 0048). */
  nameUnknown: boolean
  /** The card's own name is `?`, recorded by `new` or `identify`. It is never named later in the game. */
  ownNameUnknown: boolean
  /** Source IDs from the `new card`, `new token`, or `identify` line (`arena:106504`), for resolving an unknown name. */
  src: readonly string[]
  owner: Player
  controller: Player
  /** Null for a mana ability or a lasting effect, which are in no zone. */
  zone: Zone | null
  ended: boolean
  position: Position | null
  tapped: boolean
  face: string | null
  fused: boolean
  /** Card types, for a permanent (section 7.7). */
  types: CardTypes | null
  pt: PtValue | null
  counters: Readonly<Record<string, number>>
  attachedTo: Id | null
  exiledWith: Id | null
  adventure: Player | null
  companion: boolean
  goadedBy: readonly Player[]
  /** Choices recorded since the object was last put on the stack, or since an ability or copy was created. */
  choices: readonly Choice[]
  /** A copy still carries the original's targets; the first `target` line on it replaces them (section 7.2). */
  copiedTargets: boolean
  /** The last `cost` line's NEW since the object was cast, for check 10. */
  cost: readonly string[] | null
  abilityKind: AbilityKind | null
  source: Id | null
  text: string | null
  from: Id | null
}

export interface GameState {
  turn: number
  activePlayer: Player | null
  dayNight: 'day' | 'night' | null
  /** Top first. */
  stack: readonly Id[]
  players: Readonly<Record<Player, PlayerState>>
  objects: Readonly<Record<string, ObjectState>>
  /** An `incomplete` line was applied and no keyframe has followed yet (section 14.4). */
  incomplete: boolean
  /**
   * Entries of the keyframe being read. They are applied together by the first line after the
   * keyframe, so the state at the keyframe line is the replayed state the keyframe is compared with.
   */
  pendingKeyframe: readonly Statement[] | null
}

export interface ReplayStep {
  line: BodyLine
  state: GameState
}

const KEYFRAME_ENTRY_TYPES = new Set<Statement['type']>([
  'keyframe-player',
  'keyframe-object',
  'keyframe-effect',
  'round-clock',
])

export function initialState(game: Game): GameState {
  const players: Record<Player, PlayerState> = {}
  for (const name of ['Player1', 'Player2']) {
    const player = tagValue(game, name)
    if (player !== null) players[player] = { life: 0, poison: 0, counters: {}, librarySize: 0, pool: EMPTY_POOL }
  }
  return {
    turn: 0,
    activePlayer: null,
    dayNight: null,
    stack: [],
    players,
    objects: {},
    incomplete: false,
    pendingKeyframe: null,
  }
}

/** The state after every body line of the main line. Variation and escape lines repeat the previous state. */
export function replay(game: Game): ReplayStep[] {
  let state = initialState(game)
  const steps: ReplayStep[] = []
  for (const line of game.body) {
    state = apply(state, line)
    steps.push({ line, state })
  }
  return steps
}

export const getObject = (state: GameState, id: Id): ObjectState | undefined => state.objects[idKey(id)]

/** Applies one body line (section 14.3). Returns a new state; the old one is never mutated. */
export function apply(state: GameState, line: BodyLine): GameState {
  if (line.kind !== 'statement' || line.variation) return state
  const s = line.statement
  if (s.type === 'keyframe') return { ...state, incomplete: false, pendingKeyframe: [] }
  if (state.pendingKeyframe) {
    if (KEYFRAME_ENTRY_TYPES.has(s.type) || ((s.type === 'day' || s.type === 'night') && line.cause === null)) {
      return { ...state, pendingKeyframe: [...state.pendingKeyframe, s] }
    }
    state = resync(state, state.pendingKeyframe)
  }
  const d = new Draft(state)
  applyStatement(d, line)
  return d.finish()
}

/** A mutable copy of the state for one line. Every object it touches is replaced, never edited in place. */
class Draft {
  turn: number
  activePlayer: Player | null
  dayNight: 'day' | 'night' | null
  stack: readonly Id[]
  players: Readonly<Record<Player, PlayerState>>
  objects: Readonly<Record<string, ObjectState>>
  incomplete: boolean

  constructor(private readonly base: GameState) {
    this.turn = base.turn
    this.activePlayer = base.activePlayer
    this.dayNight = base.dayNight
    this.stack = base.stack as Id[]
    this.players = base.players
    this.objects = base.objects
    this.incomplete = base.incomplete
  }

  finish(): GameState {
    return {
      turn: this.turn,
      activePlayer: this.activePlayer,
      dayNight: this.dayNight,
      stack: this.stack,
      players: this.players,
      objects: this.objects,
      incomplete: this.incomplete,
      pendingKeyframe: null,
    }
  }

  // Collections are copied on first write, so a line that changes nothing shares them with the previous state.
  private ownObjects(): Record<string, ObjectState> {
    if (this.objects === this.base.objects) this.objects = { ...this.base.objects }
    return this.objects
  }

  private ownPlayers(): Record<Player, PlayerState> {
    if (this.players === this.base.players) this.players = { ...this.base.players }
    return this.players
  }

  object(id: Id): ObjectState | undefined {
    return this.objects[idKey(id)]
  }

  patch(id: Id, p: Partial<ObjectState>): void {
    const key = idKey(id)
    const current = this.objects[key]
    if (current) this.ownObjects()[key] = { ...current, ...p }
  }

  create(o: ObjectState): void {
    this.ownObjects()[idKey(o.id)] = o
  }

  player(name: Player): PlayerState {
    return this.players[name] ?? { life: 0, poison: 0, counters: {}, librarySize: 0, pool: EMPTY_POOL }
  }

  patchPlayer(name: Player, p: Partial<PlayerState>): void {
    this.ownPlayers()[name] = { ...this.player(name), ...p }
  }

  /** Tracked objects in one player's library, other than `except`. */
  libraryMates(owner: Player, except: Id): ObjectState[] {
    return Object.values(this.objects).filter(
      (o) => !o.ended && o.zone?.zone === 'library' && o.owner === owner && !sameId(o.id, except),
    )
  }

  /** Section 14.2: positions beyond the leaver move one closer; an unknown leaver makes every position unknown. */
  leaveLibrary(o: ObjectState, takesTop: boolean): void {
    const pos = o.position ?? (takesTop ? { end: 'top' as const, n: 1 } : null)
    for (const mate of this.libraryMates(o.owner, o.id)) {
      if (pos === null) this.patch(mate.id, { position: null })
      else if (mate.position && mate.position.end === pos.end && mate.position.n > pos.n) {
        this.patch(mate.id, { position: { end: pos.end, n: mate.position.n - 1 } })
      }
    }
    this.patchPlayer(o.owner, { librarySize: this.player(o.owner).librarySize - 1 })
  }

  /**
   * Section 14.2: an object put on the top or bottom pushes positions counted from that end one further away.
   * `nth` puts it Nth from the top instead; `random` (`bottom ?`) leaves its own position unknown.
   * An unknown position makes every position in the library unknown.
   */
  enterLibrary(o: ObjectState, end: LibraryEnd | null, nth: number | null = null, random = false): Position | null {
    // Cards in the library other than the object, before it enters.
    const others = this.player(o.owner).librarySize
    for (const mate of this.libraryMates(o.owner, o.id)) {
      const pos = mate.position
      if (end === null) this.patch(mate.id, { position: null })
      else if (!pos) continue
      else if (nth !== null) {
        // Nth from the top: cards at N or below it move down; a card counted from the bottom moves only if it is above.
        const below = pos.end === 'top' ? pos.n >= nth : others - pos.n + 1 >= nth
        if (pos.end === 'top' ? below : !below) this.patch(mate.id, { position: { end: pos.end, n: pos.n + 1 } })
      } else if (pos.end === end) {
        this.patch(mate.id, { position: { end, n: pos.n + 1 } })
      }
    }
    this.patchPlayer(o.owner, { librarySize: others + 1 })
    if (end === null || random) return null
    return nth !== null ? { end: 'top', n: nth } : { end, n: 1 }
  }

  pushStack(id: Id): void {
    this.stack = [id, ...this.stack.filter((s) => !sameId(s, id))]
  }

  removeFromStack(id: Id): void {
    this.stack = this.stack.filter((s) => !sameId(s, id))
  }

  end(id: Id): void {
    const o = this.object(id)
    if (!o) return
    this.removeFromStack(id)
    this.patch(id, { ended: true })
  }

  /** The player a cause stands for: an object's controller, or the player named. */
  causePlayer(line: StatementLine): Player | null {
    const c = line.cause
    if (!c) return null
    if (c.by === 'player') return c.player
    if (c.by === 'object' || c.by === 'cost') return this.object(c.id)?.controller ?? null
    return null
  }
}

function blankObject(id: Id, kind: ObjectKind, owner: Player, controller: Player, zone: Zone | null): ObjectState {
  return {
    id,
    kind,
    name: null,
    ownName: null,
    nameUnknown: false,
    ownNameUnknown: false,
    src: [],
    owner,
    controller,
    zone,
    ended: false,
    position: null,
    tapped: false,
    face: null,
    fused: false,
    types: null,
    pt: null,
    counters: {},
    attachedTo: null,
    exiledWith: null,
    adventure: null,
    companion: false,
    goadedBy: [],
    choices: [],
    copiedTargets: false,
    cost: null,
    abilityKind: null,
    source: null,
    text: null,
    from: null,
  }
}

const withCount = (counters: Readonly<Record<string, number>>, kind: string, n: number) => {
  const next = { ...counters }
  if (n === 0) delete next[kind]
  else next[kind] = n
  return next
}

function changePool(pool: ManaPool, mana: string[], delta: 1 | -1): ManaPool {
  const next = { ...pool }
  for (const m of mana) next[m as ManaType] = Math.max(0, next[m as ManaType] + delta)
  return next
}

/** `FRONT // BACK` names: the face that is not the front, or null when the name has one face. */
const backFace = (name: string | null): string | null => {
  const parts = name?.split(' // ') ?? []
  return parts.length > 1 ? parts.slice(1).join(' // ') : null
}
const frontFace = (name: string | null): string | null => name?.split(' // ')[0] ?? null

function applyStatement(d: Draft, line: StatementLine): void {
  const s = line.statement
  switch (s.type) {
    case 'turn':
      d.turn = s.number
      d.activePlayer = s.player
      return
    case 'incomplete':
      d.incomplete = true
      return
    case 'day':
    case 'night':
      d.dayNight = s.type
      return
    case 'new-card':
    case 'new-hidden': {
      const o = blankObject(s.id, s.type === 'new-card' ? 'card' : 'hidden', s.owner, s.owner, s.zone)
      if (s.type === 'new-card') {
        name(o, s.name)
        o.src = s.src
        enter(o, s.entering)
      }
      o.position = s.position
      if (s.zone.zone === 'stack') d.pushStack(s.id)
      d.create(o)
      return
    }
    case 'new-token': {
      const o = blankObject(s.id, 'token', s.owner, s.owner, s.zone)
      name(o, s.name)
      o.src = s.src
      enter(o, s.entering)
      d.create(o)
      return
    }
    case 'new-copy': {
      const o = blankObject(s.id, 'copy', s.owner, s.owner, s.zone)
      name(o, s.name)
      enter(o, s.entering)
      if (s.zone.zone === 'stack') {
        const original = d.object(s.of.id)
        o.choices = original?.choices ?? []
        o.copiedTargets = o.choices.some((c) => c.kind === 'target')
        d.pushStack(s.id)
      }
      d.create(o)
      return
    }
    case 'new-ability-copy': {
      const original = d.object(s.of)
      const o = blankObject(s.id, 'copy', s.controller, s.controller, { zone: 'stack' })
      if (original) {
        o.abilityKind = original.abilityKind
        o.source = original.source
        o.text = original.text
        o.choices = original.choices
        o.copiedTargets = o.choices.some((c) => c.kind === 'target')
      }
      d.pushStack(s.id)
      d.create(o)
      return
    }
    case 'new-ability': {
      const o = blankObject(s.id, 'ability', s.controller, s.controller, s.inStack ? { zone: 'stack' } : null)
      o.abilityKind = s.kind
      o.source = s.of.id
      o.text = s.text
      o.from = s.from
      if (s.inStack) d.pushStack(s.id)
      d.create(o)
      return
    }
    case 'new-effect': {
      const o = blankObject(s.id, 'effect', s.controller, s.controller, null)
      o.source = s.of.id
      o.text = s.text
      d.create(o)
      return
    }
    case 'new-emblem': {
      const o = blankObject(s.id, 'emblem', s.owner, s.owner, { zone: 'command' })
      o.text = s.text
      d.create(o)
      return
    }
    case 'play':
      move(d, line, s.object.id, { zone: 'battlefield' }, {
        entering: { face: s.face, tapped: s.tapped, attacking: null, types: s.types, pt: s.pt },
      })
      return
    case 'cast':
      move(d, line, s.object.id, { zone: 'stack' }, { face: s.face, fused: s.fused })
      return
    case 'move':
      move(d, line, s.object.id, s.to, {
        entering: s.entering,
        position: s.position,
        nth: s.nth,
        random: s.random,
        with: s.with,
        as: s.as,
      })
      return
    case 'choose': {
      const o = d.object(s.object.id)
      if (o) d.patch(o.id, { choices: [...o.choices, s.choice] })
      return
    }
    case 'target': {
      const o = d.object(s.object.id)
      if (!o) return
      const kept = o.copiedTargets ? o.choices.filter((c) => c.kind !== 'target') : o.choices
      d.patch(o.id, { choices: [...kept, { kind: 'target', target: s.target, amount: s.amount }], copiedTargets: false })
      return
    }
    case 'tap':
      d.patch(s.object.id, { tapped: true })
      return
    case 'untap':
      d.patch(s.object.id, { tapped: false })
      return
    case 'attach':
      d.patch(s.object.id, { attachedTo: s.to.id })
      return
    case 'unattach':
      d.patch(s.object.id, { attachedTo: null })
      return
    case 'control':
      d.patch(s.object.id, { controller: s.player })
      return
    case 'counters': {
      const o = d.object(s.object.id)
      if (o) d.patch(o.id, { counters: withCount(o.counters, s.kind, s.new) })
      return
    }
    case 'pt':
      // `none` means it stopped being a creature: no pt, as a keyframe entry without one (section 7.5).
      d.patch(s.object.id, { pt: s.new === 'none' ? null : s.new })
      return
    case 'types':
      d.patch(s.object.id, { types: s.new })
      return
    case 'cost':
      d.patch(s.object.id, { cost: s.new })
      return
    case 'identify': {
      // A name recorded as `?` stays `?` for the rest of the game (section 6.4): a second identify changes nothing.
      if (d.object(s.object.id)?.ownNameUnknown) return
      const unknown = s.name === null
      d.patch(s.object.id, { name: s.name, ownName: s.name, nameUnknown: unknown, ownNameUnknown: unknown, src: s.src })
      return
    }
    case 'transform': {
      const o = d.object(s.object.id)
      if (!o) return
      const showingFront = o.face === null || o.face === frontFace(o.name)
      d.patch(o.id, { face: showingFront ? backFace(o.name) : null })
      return
    }
    case 'become-copy': {
      const copied = d.object(s.of.id)
      const copiedName = s.of.name ?? copied?.name ?? null
      d.patch(s.object.id, { name: copiedName, nameUnknown: copiedName === null && (copied?.nameUnknown ?? false) })
      return
    }
    case 'goad': {
      const o = d.object(s.object.id)
      const by = d.causePlayer(line)
      if (o && by !== null && !o.goadedBy.includes(by)) d.patch(o.id, { goadedBy: [...o.goadedBy, by] })
      return
    }
    case 'end-goad': {
      const o = d.object(s.object.id)
      if (o) d.patch(o.id, { goadedBy: o.goadedBy.filter((p) => p !== s.player) })
      return
    }
    case 'reveal': {
      // Companion (section 14.3): revealed in the pregame, outside "sideboard", caused by itself.
      const o = d.object(s.object.id)
      const c = line.cause
      if (
        o &&
        d.turn === 0 &&
        o.zone?.zone === 'outside' &&
        o.zone.label === 'sideboard' &&
        c?.by === 'object' &&
        sameId(c.id, o.id)
      ) {
        d.patch(o.id, { companion: true })
      }
      return
    }
    case 'untracked':
    case 'cease':
      d.end(s.object.id)
      return
    case 'shuffle':
      for (const o of Object.values(d.objects)) {
        if (!o.ended && o.zone?.zone === 'library' && o.owner === s.player) d.patch(o.id, { ended: true })
      }
      return
    case 'life':
      d.patchPlayer(s.player, { life: s.new })
      return
    case 'poison':
      d.patchPlayer(s.player, { poison: s.new })
      return
    case 'player-counters':
      d.patchPlayer(s.player, { counters: withCount(d.player(s.player).counters, s.kind, s.new) })
      return
    case 'add':
      d.patchPlayer(s.player, { pool: changePool(d.player(s.player).pool, s.mana, 1) })
      return
    case 'pay':
    case 'lose-mana':
      d.patchPlayer(s.player, { pool: changePool(d.player(s.player).pool, s.mana, -1) })
      return
    default:
      return
  }
}

/** Sets a new object's name; null is `?` (section 6.4). */
function name(o: ObjectState, n: string | null): void {
  o.name = n
  o.ownName = n
  o.nameUnknown = n === null
  o.ownNameUnknown = n === null
}

function enter(o: ObjectState, entering: Entering): void {
  o.face = entering.face
  o.tapped = entering.tapped
  o.types = entering.types
  o.pt = entering.pt
}

interface MoveOptions {
  entering?: Entering
  face?: string | null
  fused?: boolean
  position?: LibraryEnd | null
  nth?: number | null
  random?: boolean
  with?: Id | null
  as?: string | null
}

function move(d: Draft, line: StatementLine, id: Id, to: Zone, opts: MoveOptions): void {
  const o = d.object(id)
  if (!o) return
  const from = o.zone
  const toBattlefieldFromStack = from?.zone === 'stack' && to.zone === 'battlefield'

  if (from?.zone === 'library') d.leaveLibrary(o, opts.as === 'draw' || opts.as === 'mill')
  if (from?.zone === 'stack') d.removeFromStack(id)

  // Adventure (section 14.3): cast as the second face, then exiled from the stack as it resolves.
  const onAdventure =
    from?.zone === 'stack' &&
    to.zone === 'exile' &&
    line.cause?.by === 'rule' &&
    line.cause.rule === 'resolve' &&
    o.face !== null &&
    o.face === backFace(o.name)

  // Links from other objects to this one end unless a permanent spell becomes the permanent (CR 607.2q).
  if (!toBattlefieldFromStack) {
    for (const other of Object.values(d.objects)) {
      if (other.exiledWith && sameId(other.exiledWith, id)) d.patch(other.id, { exiledWith: null })
    }
  }

  const next: Partial<ObjectState> = {
    zone: to,
    position: null,
    tapped: opts.entering?.tapped ?? false,
    face: opts.entering?.face ?? opts.face ?? null,
    fused: opts.fused ?? false,
    types: opts.entering?.types ?? null,
    pt: opts.entering?.pt ?? null,
    counters: {},
    attachedTo: null,
    exiledWith: opts.with ?? null,
    adventure: onAdventure ? o.controller : null,
    companion: false,
    goadedBy: [],
    name: from?.zone === 'battlefield' ? o.ownName : o.name,
    nameUnknown: from?.zone === 'battlefield' ? o.ownNameUnknown : o.nameUnknown,
  }
  if (to.zone === 'stack') {
    next.choices = []
    next.cost = null
    d.pushStack(id)
  }
  if (to.zone === 'library') {
    d.patch(id, next)
    const placed = d.object(id)!
    d.patch(id, { position: d.enterLibrary(placed, opts.position ?? null, opts.nth ?? null, opts.random ?? false) })
    return
  }
  d.patch(id, next)
}

/** Section 14.4: the keyframe replaces the replayed state. Unlisted live objects end, except abilities and objects outside the game. */
function resync(state: GameState, entries: readonly Statement[]): GameState {
  const d = new Draft(state)
  d.stack = []
  d.dayNight = null
  const listed = new Set<string>()
  for (const e of entries) {
    if (e.type === 'keyframe-object') listed.add(idKey(e.object.id))
    if (e.type === 'keyframe-effect') listed.add(idKey(e.id))
  }
  for (const o of Object.values(d.objects)) {
    if (o.ended || o.kind === 'ability' || o.zone?.zone === 'outside' || listed.has(idKey(o.id))) continue
    d.patch(o.id, { ended: true })
  }
  for (const player of Object.keys(d.players)) d.patchPlayer(player, { pool: EMPTY_POOL })
  for (const e of entries) {
    switch (e.type) {
      case 'keyframe-player': {
        const counters: Record<string, number> = {}
        for (const c of e.counters) counters[c.kind] = c.count
        d.patchPlayer(e.player, { life: e.life, poison: e.poison, librarySize: e.library, counters })
        break
      }
      case 'day':
      case 'night':
        d.dayNight = e.type
        break
      case 'keyframe-object': {
        const existing = d.object(e.object.id)
        const owner = e.player ?? existing?.owner ?? ''
        const o: ObjectState = existing ?? blankObject(e.object.id, 'card', owner, owner, e.zone)
        const counters: Record<string, number> = {}
        for (const c of e.counters) counters[c.kind] = c.count
        d.create({
          ...o,
          ended: false,
          name: e.object.name ?? o.name,
          ownName: o.ownName ?? e.object.name,
          zone: e.zone,
          controller: e.zone.zone === 'battlefield' && e.player ? e.player : o.controller,
          face: e.face,
          types: e.types,
          pt: e.pt,
          tapped: e.tapped,
          attachedTo: e.attached,
          exiledWith: e.with,
          adventure: e.adventure,
          companion: e.companion,
          goadedBy: e.goaded,
          counters,
          position: e.position,
          text: e.emblem ?? o.text,
          kind: e.emblem !== null ? 'emblem' : o.kind,
        })
        break
      }
      case 'keyframe-effect': {
        const existing = d.object(e.id)
        const o = existing ?? blankObject(e.id, 'effect', e.controller, e.controller, null)
        d.create({ ...o, ended: false, kind: 'effect', source: e.of.id, text: e.text, controller: e.controller })
        break
      }
      default:
        break
    }
  }
  return d.finish()
}
