import { describe, expect, it } from 'vitest'
import { stripSecrets, stripUrl } from './strip'

describe('stripSecrets', () => {
  it('removes ticket and keeps the rest', () => {
    expect(stripSecrets({ type: 'ATTACH', payload: { ticket: 'v1.x', deck: 1 } })).toEqual({
      type: 'ATTACH',
      payload: { deck: 1 },
    })
  })
  it('walks arrays and matches case-insensitively', () => {
    expect(stripSecrets([{ Email: 'a@b.c', n: 1 }, [{ AccessToken: 'x', ok: true }]])).toEqual([{ n: 1 }, [{ ok: true }]])
  })
  it('does not mutate the input', () => {
    const input = { token: 't', a: 1 }
    stripSecrets(input)
    expect(input.token).toBe('t')
  })
})

describe('stripSecrets edge cases', () => {
  it('keeps only booleans under secret keys, nested too', () => {
    expect(stripSecrets({ token: true, n: { token: 'abc', ok: 1 } })).toEqual({ token: true, n: { ok: 1 } })
  })
  it('fails closed on strings, numbers, objects and arrays', () => {
    expect(stripSecrets({ token: 'abc' })).toEqual({})
    expect(stripSecrets({ ticket: 5 })).toEqual({})
    expect(stripSecrets({ token: { value: 'abc' } })).toEqual({})
    expect(stripSecrets({ password: ['x'] })).toEqual({})
    expect(stripSecrets({ secret: null })).toEqual({})
  })
  it('matches snake_case and kebab-case keys', () => {
    const input = { access_token: 'a', 'refresh-token': 'b', id_token: 'c', session_token: 'd', 'Set-Cookie': 'e', keep: 1 }
    expect(stripSecrets(input)).toEqual({ keep: 1 })
  })
  it('removes wider secret-named keys', () => {
    const input = {
      csrfToken: 'a',
      authToken: 'b',
      wsToken: 'c',
      attachTicket: 'd',
      gameTicket: 'e',
      jwt: 'f',
      apiKey: 'g',
      sessionId: 'h',
      api_key: 'i',
      CSRF: 'j',
      xsrf: 'k',
      keep: 1,
    }
    expect(stripSecrets(input)).toEqual({ keep: 1 })
  })
  it('keeps game data that looks token-like', () => {
    const input = { isToken: true, tokens: [{ name: 'Treasure' }], authToken: 'x', n: { isToken: false } }
    expect(stripSecrets(input)).toEqual({ isToken: true, tokens: [{ name: 'Treasure' }], n: { isToken: false } })
  })
  it('keeps an own __proto__ key as data', () => {
    const out: any = stripSecrets(JSON.parse('{"__proto__":{"x":1},"a":2}'))
    expect(Object.keys(out)).toEqual(['__proto__', 'a'])
    expect(Object.getPrototypeOf(out)).toBe(Object.prototype)
  })
})

describe('stripUrl', () => {
  it('keeps the path only', () => {
    expect(stripUrl('wss://endstep.cc/ws?tz=x&t=1')).toBe('/ws')
    expect(stripUrl('https://endstep.cc/api/a#h')).toBe('/api/a')
  })
})
