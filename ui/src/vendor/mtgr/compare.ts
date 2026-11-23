import { getObject, type GameState, type ObjectState } from './replay.js'
import { formatZone, idKey, sameId, sameTypes, sameZone, type StatementFamily, type Choice, type Id, type PtValue, type Statement, type Target } from './types.js'

const show = (id: Id) => idKey(id)

const samePt = (a: PtValue | null, b: PtValue | null): boolean =>
  a === b || (typeof a === 'object' && typeof b === 'object' && a !== null && b !== null && a.power === b.power && a.toughness === b.toughness)

const sameTarget = (a: Target, b: Target): boolean =>
  a.kind === 'player' ? b.kind === 'player' && a.player === b.player : b.kind === 'object' && sameId(a.object.id, b.object.id)

export function sameChoice(a: Choice, b: Choice): boolean {
  if (a.kind !== b.kind) return false
  switch (a.kind) {
    case 'mode':
    case 'cost':
    case 'option':
    case 'type':
      return a.text === (b as typeof a).text
    case 'x':
    case 'number':
      return a.value === (b as typeof a).value
    case 'symbol':
      return a.symbol === (b as typeof a).symbol && a.payment === (b as typeof a).payment
    case 'color':
      return a.color === (b as typeof a).color
    case 'object':
      return sameId(a.object.id, (b as typeof a).object.id)
    case 'player':
      return a.player === (b as typeof a).player
    case 'name':
      return a.name === (b as typeof a).name
    case 'hidden':
      return true
    case 'target':
      return sameTarget(a.target, (b as typeof a).target) && a.amount === (b as typeof a).amount
  }
}

const sameChoices = (a: readonly Choice[], b: readonly Choice[]) =>
  a.length === b.length && a.every((c, i) => sameChoice(c, b[i]!))

const sameCounters = (a: Readonly<Record<string, number>>, b: { kind: string; count: number }[]) => {
  const keys = Object.keys(a).filter((k) => a[k] !== 0)
  return keys.length === b.length && b.every((c) => a[c.kind] === c.count)
}

const sameOptionalId = (a: Id | null, b: Id | null) => (a === null ? b === null : b !== null && sameId(a, b))

/** Whether a keyframe lists this live object (section 14.5): every zone but the stack, `outside` only for companions. */
const listedInKeyframe = (o: ObjectState): boolean => {
  if (o.ended || o.kind === 'ability') return false
  if (o.kind === 'effect') return true
  if (o.zone === null || o.zone.zone === 'stack') return false
  if (o.zone.zone === 'outside') return o.companion
  return true
}

/** Section 14.5. Returns one message per mismatch between the keyframe's entries and the state at the keyframe line. */
/** `waived` holds the families a game's `Omits` and `Partial` tags list (section 4.7): their parts of the state are not compared. */
export function compareKeyframe(state: GameState, entries: readonly Statement[], waived: ReadonlySet<StatementFamily> = new Set()): string[] {
  const out: string[] = []
  if (state.stack.length) out.push(`stack is not empty at the keyframe: ${state.stack.map(show).join(' ')}`)
  if (!waived.has('mana')) {
    for (const [player, p] of Object.entries(state.players)) {
      if (Object.values(p.pool).some((n) => n > 0)) out.push(`${player}'s mana pool is not empty at the keyframe`)
    }
  }
  const dayNight = entries.find((e) => e.type === 'day' || e.type === 'night')
  if ((dayNight?.type ?? null) !== state.dayNight) {
    out.push(`keyframe says ${dayNight?.type ?? 'neither day nor night'}, replay says ${state.dayNight ?? 'neither'}`)
  }
  for (const e of entries) {
    if (e.type !== 'keyframe-player') continue
    const p = state.players[e.player]
    if (!p) {
      out.push(`unknown player ${e.player}`)
      continue
    }
    if (p.life !== e.life) out.push(`${e.player} life: keyframe ${e.life}, replay ${p.life}`)
    if (p.poison !== e.poison) out.push(`${e.player} poison: keyframe ${e.poison}, replay ${p.poison}`)
    if (p.librarySize !== e.library) out.push(`${e.player} library: keyframe ${e.library}, replay ${p.librarySize}`)
    if (!sameCounters(p.counters, e.counters)) out.push(`${e.player} counters differ from the keyframe`)
  }

  // Section 4.7: objects in libraries and effects are not compared when the source does not record them in full.
  const compared = (inLibrary: boolean, effect: boolean) => !(inLibrary && waived.has('library')) && !(effect && waived.has('effects'))
  const expected = Object.values(state.objects)
    .filter(listedInKeyframe)
    .filter((o) => compared(o.zone?.zone === 'library', o.kind === 'effect'))
    .sort((a, b) => a.id.n - b.id.n)
  const objectEntries = entries
    .filter((e) => e.type === 'keyframe-object' || e.type === 'keyframe-effect')
    .filter((e) => compared(e.type === 'keyframe-object' && e.zone.zone === 'library', e.type === 'keyframe-effect'))
  const entryIds = objectEntries.map((e) => (e.type === 'keyframe-object' ? e.object.id : (e as { id: Id }).id))
  for (let i = 1; i < entryIds.length; i++) {
    if (entryIds[i]!.n <= entryIds[i - 1]!.n) out.push(`keyframe entries are not in increasing ID order at ${show(entryIds[i]!)}`)
  }
  const listed = new Set(entryIds.map(show))
  for (const o of expected) {
    if (!listed.has(show(o.id))) out.push(`${show(o.id)} is live in ${o.zone ? formatZone(o.zone) : 'no zone'} but the keyframe does not list it`)
  }
  const expectedKeys = new Set(expected.map((o) => show(o.id)))
  for (const e of objectEntries) {
    const id = e.type === 'keyframe-object' ? e.object.id : (e as { id: Id }).id
    const o = getObject(state, id)
    if (!o) {
      out.push(`keyframe lists ${show(id)}, which was never introduced`)
      continue
    }
    if (!expectedKeys.has(show(id))) {
      out.push(`keyframe lists ${show(id)}, which is ${o.ended ? 'ended' : `in ${o.zone ? formatZone(o.zone) : 'no zone'}`}`)
      continue
    }
    out.push(...compareEntry(o, e))
  }
  return out
}

function compareEntry(o: ObjectState, e: Statement): string[] {
  const out: string[] = []
  const id = show(o.id)
  if (e.type === 'keyframe-effect') {
    if (o.kind !== 'effect') out.push(`${id} is listed as an effect but is a ${o.kind}`)
    if (!sameOptionalId(o.source, e.of.id)) out.push(`${id} effect source differs`)
    if (o.text !== e.text) out.push(`${id} effect text differs`)
    if (o.controller !== e.controller) out.push(`${id} effect controller: keyframe ${e.controller}, replay ${o.controller}`)
    return out
  }
  if (e.type !== 'keyframe-object') return out
  if (o.kind === 'effect') out.push(`${id} is an effect but is listed as an object`)
  if (e.object.name !== o.name) out.push(`${id} name: keyframe ${JSON.stringify(e.object.name)}, replay ${JSON.stringify(o.name)}`)
  if (!o.zone || !sameZone(o.zone, e.zone)) {
    out.push(`${id} zone: keyframe ${formatZone(e.zone)}, replay ${o.zone ? formatZone(o.zone) : 'none'}`)
  }
  const z = e.zone.zone
  const expectedPlayer =
    z === 'library' || z === 'hand' || z === 'graveyard' || z === 'outside'
      ? o.owner
      : z === 'battlefield' || (z === 'command' && o.kind === 'emblem')
        ? o.controller
        : null
  if (e.player !== expectedPlayer) out.push(`${id} player: keyframe ${e.player}, replay ${expectedPlayer}`)
  if (e.face !== o.face) out.push(`${id} face: keyframe ${e.face}, replay ${o.face}`)
  if (!sameTypes(e.types, o.types)) out.push(`${id} types differ from the keyframe`)
  if (!samePt(e.pt, o.pt)) out.push(`${id} pt differs from the keyframe`)
  if (e.tapped !== o.tapped) out.push(`${id} tapped: keyframe ${e.tapped}, replay ${o.tapped}`)
  if (!sameOptionalId(e.attached, o.attachedTo)) out.push(`${id} attached differs from the keyframe`)
  if (!sameOptionalId(e.with, o.exiledWith)) out.push(`${id} with differs from the keyframe`)
  if (e.adventure !== o.adventure) out.push(`${id} adventure differs from the keyframe`)
  if (e.companion !== o.companion) out.push(`${id} companion differs from the keyframe`)
  if (e.goaded.join('\n') !== [...o.goadedBy].join('\n')) out.push(`${id} goaded differs from the keyframe`)
  if (!sameCounters(o.counters, e.counters)) out.push(`${id} counters differ from the keyframe`)
  const pos = o.position
  if ((e.position?.end ?? null) !== (pos?.end ?? null) || (e.position?.n ?? null) !== (pos?.n ?? null)) {
    out.push(`${id} position: keyframe ${e.position ? `${e.position.end} ${e.position.n}` : 'unknown'}, replay ${pos ? `${pos.end} ${pos.n}` : 'unknown'}`)
  }
  if (o.kind === 'emblem' && e.emblem !== o.text) out.push(`${id} emblem text differs from the keyframe`)
  return out
}

/** Section 14.6. Returns one message per mismatch between a stack snapshot and the replayed stack. */
export function compareStack(state: GameState, entries: readonly Statement[]): string[] {
  const out: string[] = []
  const ids = entries.map((e) => (e.type === 'stack-spell' ? e.object.id : e.type === 'stack-ability' ? e.id : null))
  if (ids.length !== state.stack.length || ids.some((id, i) => !id || !sameId(id, state.stack[i]!))) {
    out.push(`stack snapshot lists ${ids.map((i) => (i ? show(i) : '?')).join(' ') || 'nothing'}, replay has ${state.stack.map(show).join(' ') || 'nothing'}`)
    return out
  }
  for (const e of entries) {
    if (e.type === 'stack-spell') {
      const o = getObject(state, e.object.id)!
      const id = show(o.id)
      if (e.object.name !== o.name) out.push(`${id} name: snapshot ${JSON.stringify(e.object.name)}, replay ${JSON.stringify(o.name)}`)
      if (e.face !== o.face || e.fused !== o.fused) out.push(`${id} face differs from the snapshot`)
      if (e.controller !== o.controller) out.push(`${id} controller: snapshot ${e.controller}, replay ${o.controller}`)
      if (!sameChoices(o.choices, e.choices)) out.push(`${id} choices differ from the snapshot`)
    } else if (e.type === 'stack-ability') {
      const o = getObject(state, e.id)!
      const id = show(o.id)
      if (o.abilityKind !== e.kind) out.push(`${id} kind: snapshot ${e.kind}, replay ${o.abilityKind}`)
      if (!sameOptionalId(o.source, e.of.id)) out.push(`${id} source differs from the snapshot`)
      if (o.text !== e.text) out.push(`${id} text differs from the snapshot`)
      if (e.controller !== o.controller) out.push(`${id} controller: snapshot ${e.controller}, replay ${o.controller}`)
      if (!sameChoices(o.choices, e.choices)) out.push(`${id} choices differ from the snapshot`)
    }
  }
  return out
}
