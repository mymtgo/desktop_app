import type { Frame } from '../capture/format'
import { stripUrl } from '../capture/strip'
import type { HookEvent } from '../capture/units'

// Pathnames of the Endstep REST calls worth capturing.
const UUID = '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}'

export const REST_ALLOWLIST: RegExp[] = [
  new RegExp(`^/api/matches/${UUID}$`),
  new RegExp(`^/api/decks/${UUID}$`),
  new RegExp(`^/api/lobbies/${UUID}$`),
  new RegExp(`^/api/lobbies/${UUID}/seats/me/deck$`),
  /^\/api\/ranked\//,
  /^\/api\/queue/,
  /^\/api\/tournaments/,
  /^\/api\/drafts?\//,
  /^\/api\/cubes/,
  /^\/api\/me\/live$/,
  /^\/api\/auth\/me$/,
]

export interface HookTarget {
  WebSocket: typeof WebSocket
  fetch: typeof fetch
  XMLHttpRequest: typeof XMLHttpRequest
  location: { origin: string; href: string }
  // The page's document. Its baseURI honours a <base href>, which relative urls resolve against.
  document?: { baseURI: string }
}

export interface HookOptions {
  newId?: () => string
  now?: () => number
}

type Emit = (event: HookEvent) => void

// Resolves a request URL against the page origin. Returns null when it cannot be parsed.
function parseUrl(raw: string, base: string): URL | null {
  try {
    return new URL(raw, base)
  } catch {
    return null
  }
}

// The allowlisted REST path for a same-origin URL, or null.
function restPath(raw: string, origin: string, base: string): string | null {
  const url = parseUrl(raw, base)
  if (!url || url.origin !== origin) return null
  return REST_ALLOWLIST.some((pattern) => pattern.test(url.pathname)) ? url.pathname : null
}

function socketSource(raw: string, origin: string): 'game_ws' | 'lobby_ws' | null {
  // raw is the socket's resolved url (always absolute ws or wss).
  const url = parseUrl(raw, origin)
  const page = parseUrl(origin, origin)
  if (!url || !page) return null
  if (url.protocol !== 'ws:' && url.protocol !== 'wss:') return null
  if (url.host !== page.host) return null
  if (url.pathname === '/ws') return 'lobby_ws'
  if (url.pathname.startsWith('/play/')) return 'game_ws'
  return null
}

export const INSTALL_MARKER = '__mymtgoEndstepHook'

// True when the hook is already on this target. Sets the marker (not enumerable) when it is not.
function alreadyInstalled(target: HookTarget): boolean {
  try {
    if (Object.prototype.hasOwnProperty.call(target, INSTALL_MARKER)) return true
    Object.defineProperty(target, INSTALL_MARKER, { value: true })
  } catch {
    // leave the page alone; hook anyway
  }
  return false
}

export function installHooks(target: HookTarget, emit: Emit, options: HookOptions = {}): void {
  // A browser that runs the declared script again must not wrap the page's globals twice.
  if (alreadyInstalled(target)) return
  const newId = options.newId ?? (() => crypto.randomUUID())
  const now = options.now ?? (() => Date.now())
  const origin = target.location.origin
  // Relative urls resolve against the document base, read when the call happens.
  const base = () => target.document?.baseURI || target.location.href || origin

  // Nothing the extension does may reach the page.
  const safeEmit: Emit = (event) => {
    try {
      emit(event)
    } catch {
      // ignored on purpose
    }
  }

  safeEmit({ kind: 'page_start' })

  try {
    hookWebSockets(target, safeEmit, newId, now, origin)
  } catch {
    // leave the page's WebSocket alone
  }
  try {
    hookFetch(target, safeEmit, now, origin, base)
  } catch {
    // leave the page's fetch alone
  }
  try {
    hookXhr(target, safeEmit, now, origin, base)
  } catch {
    // leave the page's XMLHttpRequest alone
  }
}

interface SocketInfo {
  id: string
  src: 'game_ws' | 'lobby_ws'
  path: string
}

function hookWebSockets(
  target: HookTarget,
  emit: Emit,
  newId: () => string,
  now: () => number,
  origin: string,
) {
  const Original = target.WebSocket
  const infos = new WeakMap<object, SocketInfo>()

  class HookedWebSocket extends Original {
    constructor(...args: ConstructorParameters<typeof WebSocket>) {
      super(...args)
      try {
        const src = socketSource(String(this.url), origin)
        if (!src) return
        const info: SocketInfo = { id: newId(), src, path: stripUrl(String(this.url)) }
        infos.set(this, info)
        emit({ kind: 'socket_open', socketId: info.id, url: info.path })
        this.addEventListener('message', (event: MessageEvent) => {
          try {
            if (typeof event.data !== 'string') return
            const frame: Frame = { at: now(), src: info.src, dir: 'in', url: info.path, text: event.data }
            emit({ kind: 'frame', socketId: info.id, frame })
          } catch {
            // ignored on purpose
          }
        })
        this.addEventListener('close', () => {
          emit({ kind: 'socket_close', socketId: info.id })
        })
      } catch {
        // the page's socket works without the hook
      }
    }

    send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
      // A send that throws (for example while CONNECTING) must not be recorded.
      const at = now()
      super.send(data)
      try {
        const info = infos.get(this)
        if (info && typeof data === 'string') {
          const frame: Frame = { at, src: info.src, dir: 'out', url: info.path, text: data }
          emit({ kind: 'frame', socketId: info.id, frame })
        }
      } catch {
        // ignored on purpose
      }
    }
  }

  target.WebSocket = HookedWebSocket
}

function requestUrl(input: unknown): string {
  if (typeof input === 'string') return input
  const withUrl = input as { url?: unknown } | null
  if (withUrl && typeof withUrl.url === 'string') return withUrl.url
  return String(input)
}

function isJson(contentType: string | null | undefined): boolean {
  return typeof contentType === 'string' && contentType.toLowerCase().includes('json')
}

function hookFetch(target: HookTarget, emit: Emit, now: () => number, origin: string, base: () => string) {
  const original = target.fetch

  target.fetch = function hookedFetch(this: unknown, input: RequestInfo | URL, init?: RequestInit) {
    const promise = original.call(this ?? target, input, init)
    try {
      const path = restPath(requestUrl(input), origin, base())
      if (path) {
        // A separate branch: the page keeps the original promise.
        promise.then(
          (response) => {
            try {
              if (!isJson(response.headers?.get('content-type'))) return
              response
                .clone()
                .text()
                .then(
                  (text) => emit({ kind: 'frame', frame: { at: now(), src: 'rest', dir: 'in', url: path, text } }),
                  () => {},
                )
            } catch {
              // ignored on purpose
            }
          },
          () => {},
        )
      }
    } catch {
      // ignored on purpose
    }
    return promise
  }
}

function hookXhr(target: HookTarget, emit: Emit, now: () => number, origin: string, base: () => string) {
  const proto = target.XMLHttpRequest.prototype
  const originalOpen = proto.open
  // The allowlisted path of each request's latest open(), or null. One load listener per object.
  const paths = new WeakMap<XMLHttpRequest, string | null>()

  proto.open = function hookedOpen(this: XMLHttpRequest, ...args: unknown[]) {
    try {
      const path = restPath(requestUrl(args[1]), origin, base())
      const known = paths.has(this)
      paths.set(this, path)
      if (!known) {
        this.addEventListener('load', () => {
          try {
            const current = paths.get(this)
            if (!current) return
            if (this.responseType !== '' && this.responseType !== 'text') return
            if (!isJson(this.getResponseHeader('content-type'))) return
            emit({ kind: 'frame', frame: { at: now(), src: 'rest', dir: 'in', url: current, text: this.responseText } })
          } catch {
            // ignored on purpose
          }
        })
      }
    } catch {
      // ignored on purpose
    }
    return (originalOpen as (...a: unknown[]) => void).apply(this, args)
  } as typeof proto.open
}
