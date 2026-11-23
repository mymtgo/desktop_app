// Domain model for .mtgr 0.3.0 (spec/mtgr.md). One union member per grammar
// production, so a consumer switches on `type` instead of re-parsing text.

export type Player = string

/** An instance ID: `#12`, or `#h3` for a hypothetical object inside a variation (section 12). */
export interface Id {
  n: number
  hypothetical: boolean
}

/** `#7 "Ugin's Labyrinth"` or `#16`. `name` is null when written without one. */
export interface ObjRef {
  id: Id
  name: string | null
}

export type ZoneName =
  | 'library'
  | 'hand'
  | 'graveyard'
  | 'battlefield'
  | 'stack'
  | 'exile'
  | 'command'
  | 'ante'

export type Zone = { zone: ZoneName } | { zone: 'outside'; label: string }

export type LibraryEnd = 'top' | 'bottom'

export interface Position {
  end: LibraryEnd
  n: number
}

export interface Pt {
  power: number
  toughness: number
}

/** `2/1`, `none` (not a creature), or `?` (not reported). */
export type PtValue = Pt | 'none' | '?'

/** Mana symbols in `MANA` and `COST` notation, as written: `W`, `U`, `B`, `R`, `G`, `C`, `S`, `0`, `3`, `B/P`. */
export type Symbol = string

export type AbilityKind = 'activated' | 'triggered' | 'loyalty' | 'mana'

export type MoveAction = 'draw' | 'discard' | 'sacrifice' | 'destroy' | 'mill'

export type Step =
  | 'untap'
  | 'upkeep'
  | 'draw'
  | 'precombat-main'
  | 'beginning-of-combat'
  | 'declare-attackers'
  | 'declare-blockers'
  | 'combat-damage'
  | 'end-of-combat'
  | 'postcombat-main'
  | 'end'
  | 'cleanup'

export const STEPS: readonly Step[] = [
  'untap',
  'upkeep',
  'draw',
  'precombat-main',
  'beginning-of-combat',
  'declare-attackers',
  'declare-blockers',
  'combat-damage',
  'end-of-combat',
  'postcombat-main',
  'end',
  'cleanup',
]

export type RuleName =
  | 'opening-hand'
  | 'mulligan'
  | 'untap'
  | 'draw'
  | 'combat-damage'
  | 'cleanup'
  | 'sba'
  | 'empty-pool'
  | 'resolve'
  | 'fizzle'
  | 'no-choice'
  | 'intervening-if'
  | 'declare-attackers'
  | 'lore'
  | 'replacement-order'
  | 'duration'
  | 'delayed-trigger'
  | 'day-night'

export const RULE_NAMES: readonly RuleName[] = [
  'opening-hand',
  'mulligan',
  'untap',
  'draw',
  'combat-damage',
  'cleanup',
  'sba',
  'empty-pool',
  'resolve',
  'fizzle',
  'no-choice',
  'intervening-if',
  'declare-attackers',
  'lore',
  'replacement-order',
  'duration',
  'delayed-trigger',
  'day-night',
]

export type Cause =
  | { by: 'object'; id: Id }
  | { by: 'cost'; id: Id }
  | { by: 'player'; player: Player }
  | { by: 'rule'; rule: RuleName }
  | { by: 'source'; source: 'clock' | 'disconnect' }
  | { by: 'unknown' }

export type Target = { kind: 'object'; object: ObjRef } | { kind: 'player'; player: Player }

/** A `choose` value, also used for the choices a stack snapshot repeats (section 9.9). */
export type Choice =
  | { kind: 'mode'; text: string }
  | { kind: 'cost'; text: string }
  | { kind: 'x'; value: number }
  | { kind: 'symbol'; symbol: Symbol; payment: 'life' | Symbol }
  | { kind: 'color'; color: 'white' | 'blue' | 'black' | 'red' | 'green' }
  | { kind: 'option'; text: string }
  | { kind: 'object'; object: ObjRef }
  | { kind: 'player'; player: Player }
  | { kind: 'number'; value: number }
  | { kind: 'name'; name: string }
  | { kind: 'type'; text: string }
  | { kind: 'hidden' }
  | { kind: 'target'; target: Target; amount: number | null }

/** The card types a permanent can have (CR 110.4), in the order `TYPES` lists them (section 7.7). */
export const CARD_TYPES = ['artifact', 'battle', 'creature', 'enchantment', 'kindred', 'land', 'planeswalker'] as const
export type CardType = (typeof CARD_TYPES)[number]
/** A permanent's card types, or `?` when the source does not report them. */
export type CardTypes = readonly CardType[] | '?'

/** A spending restriction on added mana (section 9.4): its text, or unknown (`restriction ?`, decision 0042). */
export type Restriction = { text: string } | { unknown: true }

/** The words of the `Omits` and `Partial` tags (section 4.7), in the order they are written. */
export const STATEMENT_FAMILIES = ['mana', 'costs', 'priority', 'damage', 'library', 'effects'] as const
export type StatementFamily = (typeof STATEMENT_FAMILIES)[number]

/** The families an `Omits` or `Partial` tag value lists, or null when the value is not a valid list (check 1). */
export function statementFamilies(value: string | null): ReadonlySet<StatementFamily> | null {
  if (value === null) return new Set()
  const words = value.split(' ')
  let last = -1
  for (const word of words) {
    const at = STATEMENT_FAMILIES.indexOf(word as StatementFamily)
    if (at <= last) return null
    last = at
  }
  return new Set(words as StatementFamily[])
}

/** @deprecated Use `STATEMENT_FAMILIES`: `Partial` takes the same words as `Omits`. */
export const OMITTED_FAMILIES = STATEMENT_FAMILIES
/** @deprecated Use `StatementFamily`. */
export type OmittedFamily = StatementFamily
/** @deprecated Use `statementFamilies`. */
export const omittedFamilies = statementFamilies

export const sameTypes = (a: CardTypes | null, b: CardTypes | null): boolean =>
  a === b || (Array.isArray(a) && Array.isArray(b) && a.join(' ') === b.join(' '))

export interface Entering {
  face: string | null
  tapped: boolean
  attacking: Target | null
  types: CardTypes | null
  pt: Pt | null
}

export interface Printing {
  set: string
  number: string
}

export interface CounterCount {
  kind: string
  count: number
}

export type Statement =
  // Structure (sections 5.2, 9.1, 9.2, 10.5)
  | { type: 'deck'; player: Player; name: string | null; unknown: boolean }
  /** `name` is null for `?`, a card whose name the writer could not determine; `src` then holds its one source ID, and is empty otherwise (section 10.5). */
  | { type: 'deck-entry'; board: 'main' | 'side'; count: number; name: string | null; src: string[] }
  | { type: 'pregame' }
  | { type: 'turn'; number: number; player: Player }
  | { type: 'step'; step: Step }
  | { type: 'keyframe' }
  | { type: 'stack' }
  | { type: 'incomplete'; reason: string }
  | { type: 'game-draw' }
  | { type: 'day' }
  | { type: 'night' }
  | { type: 'extension'; name: string; args: (string | number | { keyword: string })[] }
  // Objects (section 6.5). A null `name` on `new-card`, `new-token`, `new-copy`, and `identify` is `?`:
  // the writer could not determine the card's name (section 6.4, decision 0048).
  | {
      type: 'new-card'
      id: Id
      name: string | null
      owner: Player
      zone: Zone
      position: Position | null
      entering: Entering
      print: Printing | null
      src: string[]
    }
  | { type: 'new-hidden'; id: Id; owner: Player; zone: Zone; position: Position | null }
  | { type: 'new-token'; id: Id; name: string | null; owner: Player; zone: Zone; entering: Entering; src: string[] }
  | { type: 'new-copy'; id: Id; of: ObjRef; name: string | null; owner: Player; zone: Zone; entering: Entering }
  | { type: 'new-ability-copy'; id: Id; of: Id; controller: Player }
  | {
      type: 'new-ability'
      id: Id
      kind: AbilityKind
      of: ObjRef
      text: string | null
      controller: Player
      inStack: boolean
      from: Id | null
    }
  | { type: 'new-effect'; id: Id; of: ObjRef; text: string | null; controller: Player }
  | { type: 'new-emblem'; id: Id; text: string | null; owner: Player }
  // Object statements (section 7)
  | { type: 'play'; object: ObjRef; from: Zone; face: string | null; tapped: boolean; types: CardTypes | null; pt: Pt | null }
  | { type: 'cast'; object: ObjRef; from: Zone; face: string | null; fused: boolean }
  | {
      type: 'move'
      object: ObjRef
      from: Zone
      to: Zone
      as: MoveAction | null
      /** `top` or `bottom` in a library (section 7.1); see `nth` and `random`. */
      position: LibraryEnd | null
      /** `top N`: N, 2 or more, for an object put Nth from the top (decision 0045). Null otherwise. */
      nth: number | null
      /** `bottom ?`: put on the bottom in a random order, at a position nobody knows (decision 0047). */
      random: boolean
      entering: Entering
      with: Id | null
    }
  | { type: 'choose'; object: ObjRef; choice: Choice }
  | { type: 'target'; object: ObjRef; target: Target; amount: number | null }
  | { type: 'tap'; object: ObjRef }
  | { type: 'untap'; object: ObjRef }
  | { type: 'attach'; object: ObjRef; to: ObjRef }
  | { type: 'unattach'; object: ObjRef }
  | { type: 'control'; object: ObjRef; player: Player }
  | { type: 'counters'; object: ObjRef; kind: string; old: number; new: number }
  | { type: 'pt'; object: ObjRef; old: PtValue; new: PtValue }
  | { type: 'types'; object: ObjRef; old: CardTypes; new: CardTypes }
  | { type: 'cost'; object: ObjRef; old: Symbol[]; new: Symbol[] }
  | { type: 'damage'; object: ObjRef; to: Target; amount: number; prevented: boolean }
  | { type: 'attack'; object: ObjRef; target: Target }
  | { type: 'block'; object: ObjRef; attacker: ObjRef }
  | { type: 'reveal'; object: ObjRef }
  | { type: 'look'; object: ObjRef; player: Player }
  | { type: 'identify'; object: ObjRef; name: string | null; print: Printing | null; src: string[] }
  | { type: 'untracked'; object: ObjRef }
  | { type: 'transform'; object: ObjRef }
  | { type: 'become-copy'; object: ObjRef; of: ObjRef }
  | { type: 'special'; object: ObjRef; action: 'companion' | 'suspend' | 'plot' }
  | { type: 'goad'; object: ObjRef }
  | { type: 'end-goad'; object: ObjRef; player: Player }
  | { type: 'cease'; object: ObjRef }
  | { type: 'resolve'; object: ObjRef }
  | { type: 'fizzle'; object: ObjRef }
  | { type: 'countered'; object: ObjRef }
  // Player statements (section 9)
  | { type: 'roll'; player: Player; sides: number | null; result: number }
  | { type: 'choose-first'; player: Player; first: Player }
  | { type: 'player-choose'; player: Player; choice: Choice }
  | { type: 'mulligan'; player: Player }
  | { type: 'keep'; player: Player }
  | { type: 'pass'; player: Player }
  | { type: 'concede'; player: Player }
  | { type: 'lose'; player: Player }
  | { type: 'win'; player: Player }
  | { type: 'shuffle'; player: Player }
  | { type: 'search'; player: Player; zone: Zone; of: Player | null }
  | { type: 'priority'; player: Player; clock: { player: Player; time: number }[] | null }
  | { type: 'add'; player: Player; mana: Symbol[]; restriction: Restriction | null }
  | { type: 'pay'; player: Player; mana: Symbol[] }
  | { type: 'lose-mana'; player: Player; mana: Symbol[] }
  | { type: 'life'; player: Player; old: number; new: number }
  | { type: 'poison'; player: Player; old: number; new: number }
  | { type: 'player-counters'; player: Player; kind: string; old: number; new: number }
  // Keyframe entries (section 10)
  | {
      type: 'keyframe-player'
      player: Player
      life: number
      poison: number
      library: number
      counters: CounterCount[]
      clock: number | null
    }
  | { type: 'round-clock'; time: number }
  | {
      type: 'keyframe-object'
      object: ObjRef
      zone: Zone
      player: Player | null
      face: string | null
      types: CardTypes | null
      pt: PtValue | null
      tapped: boolean
      attached: Id | null
      with: Id | null
      adventure: Player | null
      companion: boolean
      goaded: Player[]
      counters: CounterCount[]
      position: Position | null
      emblem: string | null
    }
  | { type: 'keyframe-effect'; id: Id; of: ObjRef; text: string | null; controller: Player }
  // Stack entries (section 9.9)
  | {
      type: 'stack-spell'
      object: ObjRef
      face: string | null
      fused: boolean
      controller: Player
      choices: Choice[]
    }
  | {
      type: 'stack-ability'
      id: Id
      kind: AbilityKind
      of: ObjRef
      text: string | null
      controller: Player
      choices: Choice[]
    }

export type StatementType = Statement['type']

export type BodyLine =
  | {
      kind: 'statement'
      line: number
      /** Seconds since the game started, or null for `-` inside a variation. */
      time: number | null
      depth: number
      statement: Statement
      cause: Cause | null
      replacements: Id[]
      nags: number[]
      braceComment: string | null
      lineComment: string | null
      /** True inside a variation (section 12). Such lines are not replayed. */
      variation: boolean
    }
  | { kind: 'escape'; line: number; text: string }
  | { kind: 'variation'; line: number; depth: number; open: boolean }

export type StatementLine = Extract<BodyLine, { kind: 'statement' }>

export interface Tag {
  name: string
  value: string
  line: number
}

export type Result = '1-0' | '0-1' | '1/2-1/2' | '*'

export interface Game {
  tags: Tag[]
  body: BodyLine[]
  result: Result
  resultLine: number
}

export interface MtgrFile {
  games: Game[]
}

/** One validation failure (section 14.1). Parse failures are check 1. */
export interface Problem {
  line: number
  check: number
  message: string
}

export interface ParseResult {
  file: MtgrFile
  problems: Problem[]
}

export const idKey = (id: Id): string => (id.hypothetical ? `#h${id.n}` : `#${id.n}`)

export const sameId = (a: Id, b: Id): boolean => a.n === b.n && a.hypothetical === b.hypothetical

export const sameZone = (a: Zone, b: Zone): boolean =>
  a.zone === b.zone && (a.zone !== 'outside' || b.zone !== 'outside' || a.label === b.label)

export const formatZone = (z: Zone): string =>
  z.zone === 'outside' ? `outside "${z.label}"` : z.zone

export const tagValue = (game: Game, name: string): string | null =>
  game.tags.find((t) => t.name === name)?.value ?? null
