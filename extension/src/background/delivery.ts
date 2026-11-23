import { CAPTURE_FORMAT_VERSION, headerLine, type Frame } from '../capture/format'
import { classify, keepUnit } from '../capture/units'
import type { CaptureStore, StoredUnit } from './store'

// Mirrors the Rust HostReply (crates/endstep-capture/src/message.rs).
export interface HostReply {
  ok: boolean
  version?: string
  error?: string
}

export interface DeliverySettings {
  extension: string
  browser: string
}

export type DropReason = 'bot' | 'spectator' | 'empty'

// A unit the filters dropped. The caller stamps the time when it records it.
export interface DropRecord {
  kind: 'match' | 'session'
  reason: DropReason
}

export interface SendReport {
  sent: string[]
  dropped: string[]
  waiting: string[]
}

// A unit up to this size goes in one message. Chrome caps one message at 64 MiB and JSON escaping
// inflates text, so bigger units go in parts (spec 4.5).
export const MAX_CONTENT_BYTES = 32 * 1024 * 1024
// Mirrors MAX_CAPTURE_BYTES in crates/endstep-capture/src/lib.rs: the helper and the app refuse
// a capture over it, so the extension keeps such a unit waiting instead of sending it.
export const MAX_CAPTURE_BYTES = 512 * 1024 * 1024
// One part stays well under the 64 MiB frame even after JSON escaping.
export const PART_BYTES = 8 * 1024 * 1024
// A line is never cut, so one line must fit in one message: JSON escaping can roughly double it
// and a frame is capped at 64 MiB.
export const MAX_LINE_BYTES = 24 * 1024 * 1024

export interface DeliveryLimits {
  maxContentBytes: number
  maxCaptureBytes: number
  partBytes: number
  maxLineBytes: number
}

const DEFAULT_LIMITS: DeliveryLimits = {
  maxContentBytes: MAX_CONTENT_BYTES,
  maxCaptureBytes: MAX_CAPTURE_BYTES,
  partBytes: PART_BYTES,
  maxLineBytes: MAX_LINE_BYTES,
}

// How units reach the helper. `send` is one message and one reply; `sendParts` sends the messages
// in order on one port (pulling each only after the previous reply) and resolves with the first
// failed reply, or the last reply.
export interface Transport {
  send: (msg: object) => Promise<HostReply>
  sendParts: (messages: Iterable<object>) => Promise<HostReply>
}

// A helper from before the parts path answers an unknown request type with this text.
const OLD_HELPER_REPLY = 'bad request'
const OLD_HELPER_ERROR = 'too large for this MyMTGO version: update MyMTGO'

// Closes every unit a crash or a restart left open (spec 4.3) and deletes frame rows whose unit
// row does not exist (a failed unit `open` leaves those). The caller then runs sendClosedUnits.
export async function recoverOnStart(store: CaptureStore): Promise<void> {
  await store.removeOrphanFrames()
  for (const unitId of await store.openUnits()) {
    await store.markClosed(unitId)
  }
}

// Sends every closed unit to the helper. A unit is removed only after the helper confirms it,
// or when the filters drop it (then `recordDrop` is told why); anything else keeps it with its
// error for the next pass.
export async function sendClosedUnits(
  store: CaptureStore,
  transport: Transport,
  settings: DeliverySettings,
  recordDrop: (drop: DropRecord) => Promise<void>,
  limits: DeliveryLimits = DEFAULT_LIMITS,
): Promise<SendReport> {
  const report: SendReport = { sent: [], dropped: [], waiting: [] }
  for (const unit of await store.closedUnits()) {
    const outcome = await deliverUnit(store, unit, transport, settings, recordDrop, limits)
    report[outcome].push(unit.unitId)
  }
  return report
}

async function deliverUnit(
  store: CaptureStore,
  unit: StoredUnit,
  transport: Transport,
  settings: DeliverySettings,
  recordDrop: (drop: DropRecord) => Promise<void>,
  limits: DeliveryLimits,
): Promise<keyof SendReport> {
  try {
    const lines = await store.lines(unit.unitId)
    const result =
      unit.unitKind === 'session'
        ? { classification: 'unknown' as const, spectator: false }
        : classify(framesFromLines(lines))
    const kept = keepUnit(unit.unitKind, result, lines.length)
    if (!kept) {
      await store.remove(unit.unitId)
      const reason: DropReason = unit.unitKind === 'session' ? 'empty' : result.spectator ? 'spectator' : 'bot'
      // Recording is for the popup only; a failure must not turn a drop into a retry.
      await recordDrop({ kind: unit.unitKind, reason }).catch((error: unknown) =>
        console.warn('mymtgo-endstep: could not record a dropped unit', error),
      )
      return 'dropped'
    }

    const header = headerLine({
      mymtgo_endstep_capture: CAPTURE_FORMAT_VERSION,
      unit_id: unit.unitId,
      kind: unit.unitKind,
      started_at: unit.startedAt,
      counter: unit.counter,
      extension: settings.extension,
      browser: settings.browser,
      developer: false,
      classification: result.classification,
    })
    const all = [header, ...lines]
    const { size, longestLine } = measure(all)
    const inParts = size > limits.maxContentBytes
    if (size > limits.maxCaptureBytes || (inParts && longestLine > limits.maxLineBytes)) {
      await store.setError(unit.unitId, 'too large')
      return 'waiting'
    }

    const reply = inParts
      ? await transport.sendParts(partMessages(unit.unitId, all, limits.partBytes))
      : await transport.send({ type: 'deliver', unit_id: unit.unitId, content: all.join('\n') })
    if (reply?.ok === true) {
      await store.remove(unit.unitId)
      return 'sent'
    }
    const refused = inParts && reply?.error === OLD_HELPER_REPLY
    await store.setError(unit.unitId, refused ? OLD_HELPER_ERROR : (reply?.error ?? 'the helper refused the unit'))
    return 'waiting'
  } catch (error) {
    await store.setError(unit.unitId, errorText(error)).catch(() => {})
    return 'waiting'
  }
}

// Rebuilds Frame-shaped input for classify from stored lines: `text` is the line's `body` as
// JSON, or its `text`. The fields classify reads survive stripping. Unreadable lines are skipped.
export function framesFromLines(lines: string[]): Frame[] {
  const frames: Frame[] = []
  for (const line of lines) {
    let parsed: unknown
    try {
      parsed = JSON.parse(line)
    } catch {
      continue
    }
    if (typeof parsed !== 'object' || parsed === null) continue
    const { at, src, dir, url, body, text } = parsed as Record<string, unknown>
    if (typeof at !== 'number' || typeof url !== 'string') continue
    if (src !== 'game_ws' && src !== 'lobby_ws' && src !== 'rest') continue
    if (dir !== 'in' && dir !== 'out') continue
    const frameText = 'body' in (parsed as object) ? JSON.stringify(body) : typeof text === 'string' ? text : null
    if (frameText === null) continue
    frames.push({ at, src, dir, url, text: frameText })
  }
  return frames
}

function byteLength(text: string): number {
  return new TextEncoder().encode(text).byteLength
}

// The UTF-8 size of `lines.join('\n')`, counted without building the joined string (a unit can be
// near the JS string length limit), and the size of its longest line.
function measure(lines: string[]): { size: number; longestLine: number } {
  let size = Math.max(lines.length - 1, 0)
  let longestLine = 0
  for (const line of lines) {
    const bytes = byteLength(line)
    size += bytes
    longestLine = Math.max(longestLine, bytes)
  }
  return { size, longestLine }
}

// Yields a unit's lines as parts of whole lines, one at a time. The parts joined together are
// exactly `lines.join('\n')`: every part but the last ends with the newline that separates it from
// the next. A line is never cut (a slice could split a surrogate pair), so a line longer than
// `maxPartBytes` is a part of its own.
function* partsOf(lines: string[], maxPartBytes: number): Generator<string> {
  let group: string[] = []
  let groupBytes = 0
  for (const line of lines) {
    const lineBytes = byteLength(line) + 1
    if (group.length > 0 && groupBytes + lineBytes > maxPartBytes) {
      yield group.join('\n') + '\n'
      group = []
      groupBytes = 0
    }
    group.push(line)
    groupBytes += lineBytes
  }
  yield group.join('\n')
}

export function splitIntoParts(lines: string[], maxPartBytes: number): string[] {
  return [...partsOf(lines, maxPartBytes)]
}

// The messages for a unit in parts, built as they are pulled so a big unit is never held twice.
// `deliver_end` comes last because it carries the part count.
function* partMessages(unitId: string, lines: string[], partBytes: number): Generator<object> {
  let count = 0
  for (const content of partsOf(lines, partBytes)) {
    yield { type: 'deliver_part', unit_id: unitId, index: count, content }
    count += 1
  }
  yield { type: 'deliver_end', unit_id: unitId, parts: count }
}

export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message
  return String(error)
}
