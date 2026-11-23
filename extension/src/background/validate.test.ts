import { describe, expect, it } from 'vitest'
import { parseHookMessage, parsePopupMessage } from './validate'

const PAGE = { url: 'https://endstep.cc/play/abc', hasTab: true }
const EXT = 'chrome-extension://abc/'
const POPUP = { url: 'chrome-extension://abc/popup.html', hasTab: false }
const frame = { at: 5, src: 'game_ws', dir: 'in', url: '/play/abc/ws', text: '{}' }

describe('parseHookMessage', () => {
  it('accepts_each_known_event_from_endstep', () => {
    expect(parseHookMessage({ endstep: { kind: 'page_start' } }, PAGE)).toEqual({ kind: 'page_start' })
    expect(parseHookMessage({ endstep: { kind: 'socket_open', socketId: 's1', url: 'wss://endstep.cc/ws' } }, PAGE)).toEqual(
      { kind: 'socket_open', socketId: 's1', url: 'wss://endstep.cc/ws' },
    )
    expect(parseHookMessage({ endstep: { kind: 'socket_close', socketId: 's1' } }, PAGE)).toEqual({
      kind: 'socket_close',
      socketId: 's1',
    })
    expect(parseHookMessage({ endstep: { kind: 'frame', socketId: 's1', frame } }, PAGE)).toEqual({
      kind: 'frame',
      socketId: 's1',
      frame,
    })
    expect(parseHookMessage({ endstep: { kind: 'frame', frame: { ...frame, src: 'rest' } } }, PAGE)).toEqual({
      kind: 'frame',
      frame: { ...frame, src: 'rest' },
    })
  })

  it('drops_extra_fields', () => {
    const parsed = parseHookMessage({ endstep: { kind: 'frame', frame: { ...frame, extra: 1 }, other: 2 } }, PAGE)
    expect(parsed).toEqual({ kind: 'frame', frame })
  })

  it('refuses_other_senders', () => {
    const message = { endstep: { kind: 'page_start' } }
    const from = (url?: string, tabUrl?: string, hasTab = true) => parseHookMessage(message, { url, tabUrl, hasTab })
    expect(from(undefined, undefined)).toBeNull()
    expect(from('http://endstep.cc/')).toBeNull()
    expect(from('https://endstep.cc.evil.example/')).toBeNull()
    expect(from('https://evil.example/https://endstep.cc/')).toBeNull()
    expect(from('https://endstep.cc/', 'https://evil.example/')).toBeNull()
    expect(from('https://evil.example/', 'https://endstep.cc/')).toBeNull()
    expect(from('https://endstep.cc/', undefined, false)).toBeNull()
    expect(from('https://endstep.cc/')).toEqual({ kind: 'page_start' })
    expect(from(undefined, 'https://endstep.cc/x')).toEqual({ kind: 'page_start' })
    expect(from('https://endstep.cc/a', 'https://endstep.cc/b')).toEqual({ kind: 'page_start' })
  })

  it('refuses_malformed_events', () => {
    const bad: unknown[] = [
      null,
      'x',
      {},
      { endstep: null },
      { endstep: { kind: 'nope' } },
      { endstep: { kind: 'socket_open', socketId: 1, url: 'x' } },
      { endstep: { kind: 'socket_open', socketId: 's' } },
      { endstep: { kind: 'socket_close' } },
      { endstep: { kind: 'frame', frame: null } },
      { endstep: { kind: 'frame', socketId: 3, frame } },
      { endstep: { kind: 'frame', frame: { ...frame, at: '5' } } },
      { endstep: { kind: 'frame', frame: { ...frame, at: Number.NaN } } },
      { endstep: { kind: 'frame', frame: { ...frame, src: 'other' } } },
      { endstep: { kind: 'frame', frame: { ...frame, dir: 'up' } } },
      { endstep: { kind: 'frame', frame: { ...frame, url: 1 } } },
      { endstep: { kind: 'frame', frame: { ...frame, text: {} } } },
    ]
    for (const message of bad) expect(parseHookMessage(message, PAGE)).toBeNull()
  })
})

describe('parsePopupMessage', () => {
  it('accepts_popup_requests_from_the_extension_only', () => {
    expect(parsePopupMessage({ popup: 'status' }, POPUP, EXT)).toEqual({ popup: 'status' })
    expect(parsePopupMessage({ popup: 'setEnabled', value: false }, POPUP, EXT)).toEqual({ popup: 'setEnabled', value: false })
    expect(parsePopupMessage({ popup: 'resetDropped' }, POPUP, EXT)).toEqual({ popup: 'resetDropped' })
    expect(parsePopupMessage({ popup: 'resetDropped' }, { ...POPUP, hasTab: true }, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'setKeepBots', value: true }, POPUP, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'status' }, { ...POPUP, hasTab: true }, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'status' }, { hasTab: false }, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'status' }, { url: 'https://endstep.cc/', hasTab: false }, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'status' }, { url: 'chrome-extension://other/popup.html', hasTab: false }, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'status' }, POPUP, '')).toBeNull()
    expect(parsePopupMessage({ popup: 'setEnabled', value: 'yes' }, POPUP, EXT)).toBeNull()
    expect(parsePopupMessage({ popup: 'other' }, POPUP, EXT)).toBeNull()
    expect(parsePopupMessage({ endstep: { kind: 'page_start' } }, POPUP, EXT)).toBeNull()
  })
})
