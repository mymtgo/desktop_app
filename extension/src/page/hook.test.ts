import { describe, expect, it } from 'vitest'
import { installHooks, type HookTarget } from './hook'
import type { HookEvent } from '../capture/units'

const ORIGIN = 'https://endstep.cc'

class FakeSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  url: string
  sent: unknown[] = []
  onmessage: ((event: { data: unknown }) => void) | null = null
  private listeners: Record<string, Array<(event: { data?: unknown }) => void>> = {}
  constructor(url: string | URL) {
    // Like a browser: relative or http(s) input resolves to an absolute ws or wss url.
    const resolved = new URL(String(url), 'https://endstep.cc/')
    if (resolved.protocol === 'https:') resolved.protocol = 'wss:'
    if (resolved.protocol === 'http:') resolved.protocol = 'ws:'
    this.url = resolved.href
  }
  addEventListener(type: string, fn: (event: { data?: unknown }) => void) {
    ;(this.listeners[type] ??= []).push(fn)
  }
  failSend = false
  send(data: unknown) {
    if (this.failSend) throw new Error('InvalidStateError')
    this.sent.push(data)
  }
  // Test helper: deliver an event the way a browser would.
  fire(type: string, data?: unknown) {
    for (const fn of this.listeners[type] ?? []) fn({ data })
    if (type === 'message') this.onmessage?.({ data })
  }
}

class FakeXhr {
  responseType = ''
  responseText = ''
  contentType: string | null = 'application/json'
  getResponseHeader(_name: string) {
    return this.contentType
  }
  openedWith: unknown[] = []
  private listeners: Record<string, Array<() => void>> = {}
  open(...args: unknown[]) {
    this.openedWith = args
  }
  addEventListener(type: string, fn: () => void) {
    ;(this.listeners[type] ??= []).push(fn)
  }
  fire(type: string) {
    for (const fn of this.listeners[type] ?? []) fn()
  }
}

interface FakeTarget {
  WebSocket: typeof FakeSocket
  XMLHttpRequest: typeof FakeXhr
  fetch: (input: unknown) => Promise<any>
  location: { origin: string; href: string }
  document: { baseURI: string }
}

function makeTarget(fakeFetch?: (input: unknown) => Promise<unknown>): FakeTarget {
  // The hook patches the XHR prototype, so each target gets its own subclass and tests stay apart.
  class TargetXhr extends FakeXhr {}
  class TargetSocket extends FakeSocket {}
  return {
    WebSocket: TargetSocket,
    XMLHttpRequest: TargetXhr,
    fetch: fakeFetch ?? (async () => ({})),
    location: { origin: ORIGIN, href: `${ORIGIN}/` },
    document: { baseURI: `${ORIGIN}/` },
  }
}

const asHookTarget = (target: FakeTarget) => target as unknown as HookTarget

function setup(fakeFetch?: (input: unknown) => Promise<unknown>) {
  const target = makeTarget(fakeFetch)
  const events: HookEvent[] = []
  let n = 0
  installHooks(asHookTarget(target), (event) => events.push(event), { newId: () => `s${++n}`, now: () => 42 })
  return { target, events }
}

const frames = (events: HookEvent[]) =>
  events.flatMap((e) => (e.kind === 'frame' ? [e] : []))

const tick = () => new Promise((resolve) => setTimeout(resolve, 0))

function fakeResponse(text: string, contentType = 'application/json; charset=utf-8') {
  const response = {
    headers: { get: (name: string) => (name === 'content-type' ? contentType : null) },
    clone: () => ({ text: async () => text }),
  }
  return response
}

describe('installHooks', () => {
  it('emits page_start once on install', () => {
    const { events } = setup()
    expect(events).toEqual([{ kind: 'page_start' }])
  })

  it('a_second_hook_install_does_nothing', () => {
    const target = makeTarget()
    const events: HookEvent[] = []
    installHooks(asHookTarget(target), (event) => events.push(event))
    const wrapped = target.WebSocket
    const wrappedFetch = target.fetch
    const wrappedXhr = target.XMLHttpRequest
    const wrappedOpen = target.XMLHttpRequest.prototype.open
    installHooks(asHookTarget(target), (event) => events.push(event))
    expect(target.WebSocket).toBe(wrapped)
    expect(target.fetch).toBe(wrappedFetch)
    expect(target.XMLHttpRequest).toBe(wrappedXhr)
    expect(target.XMLHttpRequest.prototype.open).toBe(wrappedOpen)
    expect(events).toEqual([{ kind: 'page_start' }])
  })

  it('the_install_marker_is_not_enumerable', () => {
    const { target } = setup()
    expect(Object.keys(target)).not.toContain('__mymtgoEndstepHook')
  })

  it('socket_frames_are_emitted_both_ways', () => {
    const { target, events } = setup()
    const ws = new target.WebSocket('wss://endstep.cc/play/abc123')
    ws.send('{"type":"MOVE"}')
    ws.fire('message', '{"type":"STATE"}')
    ws.fire('message', new ArrayBuffer(4))
    ws.fire('close')
    expect(ws.sent).toEqual(['{"type":"MOVE"}'])
    expect(events.slice(1)).toEqual([
      { kind: 'socket_open', socketId: 's1', url: '/play/abc123' },
      {
        kind: 'frame',
        socketId: 's1',
        frame: { at: 42, src: 'game_ws', dir: 'out', url: '/play/abc123', text: '{"type":"MOVE"}' },
      },
      {
        kind: 'frame',
        socketId: 's1',
        frame: { at: 42, src: 'game_ws', dir: 'in', url: '/play/abc123', text: '{"type":"STATE"}' },
      },
      { kind: 'socket_close', socketId: 's1' },
    ])
  })

  it('a_lobby_socket_url_loses_its_query', () => {
    const { target, events } = setup()
    const ws = new target.WebSocket('wss://endstep.cc/ws?ticket=SECRET')
    ws.fire('message', '{"type":"HELLO"}')
    const all = JSON.stringify(events)
    expect(all).not.toContain('SECRET')
    expect(events[1]).toEqual({ kind: 'socket_open', socketId: 's1', url: '/ws' })
    expect(frames(events)[0].frame.src).toBe('lobby_ws')
    expect(frames(events)[0].frame.url).toBe('/ws')
  })

  it('other_sockets_are_not_recorded', () => {
    const { target, events } = setup()
    const other = new target.WebSocket('wss://example.com/ws')
    other.send('x')
    other.fire('message', 'y')
    const wrongPath = new target.WebSocket('wss://endstep.cc/other')
    wrongPath.fire('message', 'z')
    expect(events).toEqual([{ kind: 'page_start' }])
    expect(other.sent).toEqual(['x'])
  })

  it('the_page_still_gets_every_message_when_emit_throws', () => {
    const target = makeTarget()
    let armed = false
    installHooks(
      asHookTarget(target),
      () => {
        if (armed) throw new Error('extension broke')
      },
      { newId: () => 's1', now: () => 1 },
    )
    armed = true
    const ws = new target.WebSocket('wss://endstep.cc/play/abc')
    const got: unknown[] = []
    ws.onmessage = (event) => got.push(event.data)
    ws.send('out')
    ws.fire('message', 'in')
    expect(ws.sent).toEqual(['out'])
    expect(got).toEqual(['in'])
  })

  it('wrapped_sockets_are_still_instances', () => {
    const target = makeTarget()
    const Original = target.WebSocket
    installHooks(asHookTarget(target), () => {}, { newId: () => 's1' })
    const ws = new target.WebSocket('wss://endstep.cc/play/abc')
    expect(ws instanceof Original).toBe(true)
    expect(target.WebSocket.OPEN).toBe(1)
    expect(target.WebSocket.prototype instanceof Original).toBe(true)
  })

  it('allowlisted_fetch_bodies_are_emitted_and_the_response_is_untouched', async () => {
    const response = fakeResponse('{"id":"m1"}')
    const { target, events } = setup(async () => response)
    const result = await target.fetch('https://endstep.cc/api/matches/11111111-1111-4111-8111-111111111111?x=1')
    expect(result).toBe(response)
    await tick()
    expect(frames(events)).toEqual([
      {
        kind: 'frame',
        frame: { at: 42, src: 'rest', dir: 'in', url: '/api/matches/11111111-1111-4111-8111-111111111111', text: '{"id":"m1"}' },
      },
    ])
  })

  it('the site-wide live matches list is not emitted but a match by uuid is', async () => {
    const { target, events } = setup(async () => fakeResponse('[]'))
    await target.fetch('/api/matches/live')
    await target.fetch('/api/matches/11111111-1111-4111-8111-111111111111')
    await tick()
    expect(frames(events).map((e) => e.frame.url)).toEqual(['/api/matches/11111111-1111-4111-8111-111111111111'])
  })

  it('fetch_accepts_request_and_url_objects', async () => {
    const { target, events } = setup(async () => fakeResponse('{}'))
    await target.fetch(new URL('/api/auth/me', ORIGIN))
    await target.fetch({ url: `${ORIGIN}/api/cubes` } as unknown as Request)
    await target.fetch('/api/me/live')
    await tick()
    expect(frames(events).map((e) => e.frame.url)).toEqual(['/api/auth/me', '/api/cubes', '/api/me/live'])
  })

  it('other_fetches_are_not_emitted', async () => {
    const { target, events } = setup(async () => fakeResponse('{}'))
    await target.fetch('https://endstep.cc/api/unknown')
    await target.fetch('https://example.com/api/matches/11111111-1111-4111-8111-111111111111')
    await tick()
    expect(frames(events)).toEqual([])
  })

  it('fetch_failures_in_the_hook_are_swallowed', async () => {
    const response = {
      clone: () => {
        throw new Error('x')
      },
    }
    const { target } = setup(async () => response)
    expect(await target.fetch('/api/auth/me')).toBe(response)
    const broken = { clone: () => ({ text: () => Promise.reject(new Error('x')) }) }
    const second = setup(async () => broken)
    expect(await second.target.fetch('/api/auth/me')).toBe(broken)
    await tick()
  })

  it('a_rejected_fetch_still_rejects_for_the_page', async () => {
    const { target } = setup(() => Promise.reject(new Error('offline')))
    await expect(target.fetch('/api/auth/me')).rejects.toThrow('offline')
  })

  it('allowlisted_xhr_bodies_are_emitted', () => {
    const { target, events } = setup()
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', '/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd?token=abc', true)
    xhr.responseText = '{"deck":1}'
    xhr.fire('load')
    expect(xhr.openedWith).toEqual(['GET', '/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd?token=abc', true])
    expect(frames(events)).toEqual([
      {
        kind: 'frame',
        frame: { at: 42, src: 'rest', dir: 'in', url: '/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd', text: '{"deck":1}' },
      },
    ])
  })

  it('xhr_skips_other_urls_and_binary_response_types', () => {
    const { target, events } = setup()
    const other = new target.XMLHttpRequest()
    other.open('GET', '/api/unknown')
    other.fire('load')
    const binary = new target.XMLHttpRequest()
    binary.open('GET', '/api/auth/me')
    binary.responseType = 'arraybuffer'
    binary.fire('load')
    expect(frames(events)).toEqual([])
  })

  it('classifies_sockets_by_their_resolved_url', () => {
    const { target, events } = setup()
    const lobby = new target.WebSocket('/ws')
    const game = new target.WebSocket('https://endstep.cc/play/x/ws')
    lobby.fire('message', 'a')
    game.fire('message', 'b')
    expect(events.filter((e) => e.kind === 'socket_open')).toEqual([
      { kind: 'socket_open', socketId: 's1', url: '/ws' },
      { kind: 'socket_open', socketId: 's2', url: '/play/x/ws' },
    ])
    expect(frames(events).map((e) => e.frame.src)).toEqual(['lobby_ws', 'game_ws'])
  })

  it('a_throwing_send_records_nothing_and_reaches_the_caller', () => {
    const { target, events } = setup()
    const ws = new target.WebSocket('wss://endstep.cc/play/abc')
    ws.failSend = true
    expect(() => ws.send('x')).toThrow('InvalidStateError')
    expect(frames(events)).toEqual([])
  })

  it('non_json_fetch_responses_are_not_read', async () => {
    const { target, events } = setup(async () => fakeResponse('data: x', 'text/event-stream'))
    await target.fetch('/api/me/live')
    await tick()
    expect(frames(events)).toEqual([])
  })

  it('non_json_xhr_responses_are_not_emitted', () => {
    const { target, events } = setup()
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', '/api/auth/me')
    xhr.contentType = 'text/html'
    xhr.fire('load')
    expect(frames(events)).toEqual([])
  })

  it('a_reopened_xhr_emits_once_under_its_latest_path', () => {
    const { target, events } = setup()
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', '/api/auth/me')
    xhr.open('GET', '/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd')
    xhr.responseText = '{}'
    xhr.fire('load')
    expect(frames(events).map((e) => e.frame.url)).toEqual(['/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd'])
    xhr.open('GET', '/api/unknown')
    xhr.fire('load')
    expect(frames(events)).toHaveLength(1)
  })

  it('xhr_urls_resolve_against_the_document_base', () => {
    const { target, events } = setup()
    // A <base href> changes baseURI while the page's own location stays put.
    target.document.baseURI = 'https://endstep.cc/api/'
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', 'decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd')
    xhr.responseText = '{}'
    xhr.fire('load')
    expect(frames(events).map((e) => e.frame.url)).toEqual(['/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd'])
  })

  it('relative_urls_fall_back_to_the_location_without_a_document', () => {
    const target = makeTarget() as Partial<FakeTarget> & Omit<FakeTarget, 'document'>
    delete target.document
    const events: HookEvent[] = []
    installHooks(asHookTarget(target as FakeTarget), (event) => events.push(event))
    target.location.href = 'https://endstep.cc/api/'
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', 'auth/me')
    xhr.responseText = '{}'
    xhr.fire('load')
    expect(frames(events).map((e) => e.frame.url)).toEqual(['/api/auth/me'])
  })

  it.each([
    '/api/decks/not-a-uuid',
    '/api/decks/dddddddd-dddd-4ddd-8ddd-dddddddddddd/extra',
    '/api/decks/',
    '/api/lobbies/not-a-uuid',
    '/api/lobbies/dddddddd-dddd-4ddd-8ddd-dddddddddddd/other',
    '/api/lobbies/not-a-uuid/seats/me/deck',
  ])('a_non_uuid_deck_or_lobby_path_is_not_captured: %s', (path) => {
    const { target, events } = setup()
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', path)
    xhr.responseText = '{}'
    xhr.fire('load')
    expect(frames(events)).toEqual([])
  })

  it('a_lobby_uuid_path_is_captured', () => {
    const { target, events } = setup()
    const xhr = new target.XMLHttpRequest()
    xhr.open('GET', '/api/lobbies/dddddddd-dddd-4ddd-8ddd-dddddddddddd/seats/me/deck')
    xhr.responseText = '{}'
    xhr.fire('load')
    expect(frames(events)).toHaveLength(1)
  })
})
