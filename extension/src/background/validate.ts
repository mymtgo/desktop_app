import type { Frame } from '../capture/format'
import type { HookEvent } from '../capture/units'

// Hook events are only taken from content scripts on Endstep pages.
const ENDSTEP_ORIGIN = 'https://endstep.cc/'

const SOURCES: Frame['src'][] = ['game_ws', 'lobby_ws', 'rest']
const DIRECTIONS: Frame['dir'][] = ['in', 'out']

export type PopupRequest =
  | { popup: 'status' }
  | { popup: 'setEnabled'; value: boolean }
  | { popup: 'resetDropped' }

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function parseFrame(value: unknown): Frame | null {
  if (!isObject(value)) return null
  const { at, src, dir, url, text } = value
  if (typeof at !== 'number' || !Number.isFinite(at)) return null
  if (!SOURCES.includes(src as Frame['src'])) return null
  if (!DIRECTIONS.includes(dir as Frame['dir'])) return null
  if (typeof url !== 'string' || typeof text !== 'string') return null
  return { at, src: src as Frame['src'], dir: dir as Frame['dir'], url, text }
}

// What the background knows about a message's sender: `url` is the sending frame
// (always set for content scripts), `tabUrl` the tab's (needs host or tabs permission).
export interface SenderInfo {
  url?: string
  tabUrl?: string
  hasTab: boolean
}

// True when the sender is an Endstep page: at least one url is known and every known one is on Endstep.
export function fromEndstep(sender: SenderInfo): boolean {
  const urls = [sender.url, sender.tabUrl].filter((u) => u !== undefined)
  if (!sender.hasTab || urls.length === 0) return false
  return urls.every((u) => typeof u === 'string' && u.startsWith(ENDSTEP_ORIGIN))
}

// The relay's `{ endstep: event }` message as a HookEvent, or null when the sender is not an
// Endstep page or the event is not well formed. Returns a fresh object holding only known fields.
export function parseHookMessage(message: unknown, sender: SenderInfo): HookEvent | null {
  if (!fromEndstep(sender)) return null
  if (!isObject(message) || !isObject(message.endstep)) return null
  const event = message.endstep

  if (event.kind === 'page_start') return { kind: 'page_start' }
  if (event.kind === 'socket_open') {
    if (typeof event.socketId !== 'string' || typeof event.url !== 'string') return null
    return { kind: 'socket_open', socketId: event.socketId, url: event.url }
  }
  if (event.kind === 'socket_close') {
    if (typeof event.socketId !== 'string') return null
    return { kind: 'socket_close', socketId: event.socketId }
  }
  if (event.kind === 'frame') {
    const frame = parseFrame(event.frame)
    if (!frame) return null
    if (event.socketId === undefined) return { kind: 'frame', frame }
    if (typeof event.socketId !== 'string') return null
    return { kind: 'frame', socketId: event.socketId, frame }
  }
  return null
}

// A popup request, or null. Popup messages come from the extension's own pages (`extensionUrl` is
// chrome.runtime.getURL('')), never from a tab.
export function parsePopupMessage(message: unknown, sender: SenderInfo, extensionUrl: string): PopupRequest | null {
  if (sender.hasTab || typeof sender.url !== 'string' || extensionUrl === '') return null
  if (!sender.url.startsWith(extensionUrl) || !isObject(message)) return null
  if (message.popup === 'status') return { popup: 'status' }
  if (message.popup === 'resetDropped') return { popup: 'resetDropped' }
  if (message.popup === 'setEnabled') {
    if (typeof message.value !== 'boolean') return null
    return { popup: message.popup, value: message.value }
  }
  return null
}

// The background's answer to every popup request.
export interface PopupStatus {
  captureEnabled: boolean
  // Set when IndexedDB could not be opened (Firefox "never remember history" mode).
  storageError: string | null
  // The last failed write of a unit action (open, append, close), if any.
  lastStoreError: { message: string; at: number } | null
  recording: number
  waiting: { unitId: string; kind: 'match' | 'session'; startedAt: number; error: string | null }[]
  lastDeliveredAt: number | null
  // Units the filters dropped (bot, spectator, empty session), and the last one.
  dropped: { count: number; last: { kind: 'match' | 'session'; reason: 'bot' | 'spectator' | 'empty'; at: number } | null }
  helper: { ok: boolean; version: string | null; error: string | null }
  // False when the browser has taken the endstep.cc host permission away (Firefox lets users do it).
  hostAccess: boolean
  // True when the active endstep.cc tab has no page hook yet: it was open before the install and needs a reload.
  tabNeedsReload: boolean
}
