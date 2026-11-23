import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { HostReply } from './delivery'
import { sendParts, type PortLike } from './port'

class FakePort implements PortLike {
  posted: object[] = []
  disconnects = 0
  throwOnPost = false
  private messageListeners: ((reply: HostReply) => void)[] = []
  private disconnectListeners: (() => void)[] = []
  onMessage = { addListener: (fn: (reply: HostReply) => void) => void this.messageListeners.push(fn) }
  onDisconnect = { addListener: (fn: () => void) => void this.disconnectListeners.push(fn) }
  postMessage = (message: object) => {
    if (this.throwOnPost) throw new Error('port is closed')
    this.posted.push(message)
  }
  disconnect = () => {
    this.disconnects += 1
  }
  reply(reply: HostReply) {
    for (const fn of this.messageListeners) fn(reply)
  }
  drop() {
    for (const fn of this.disconnectListeners) fn()
  }
}

const makePort = () => new FakePort()

const messages = () => [{ n: 0 }, { n: 1 }, { n: 2 }]

async function flush() {
  await Promise.resolve()
  await Promise.resolve()
}

describe('sendParts', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('resolves_only_after_the_reply_to_the_last_message', async () => {
    const port = makePort()
    let settled = false
    const result = sendParts(() => port, messages(), 1000, () => undefined).then((reply) => {
      settled = true
      return reply
    })
    expect(port.posted).toEqual([{ n: 0 }])
    port.reply({ ok: true })
    port.reply({ ok: true })
    await flush()
    expect(port.posted).toEqual([{ n: 0 }, { n: 1 }, { n: 2 }])
    expect(settled).toBe(false)
    port.reply({ ok: true, version: 'v' })
    expect(await result).toEqual({ ok: true, version: 'v' })
    expect(port.disconnects).toBe(1)
  })

  it('a_failed_reply_resolves_with_it_and_posts_nothing_more', async () => {
    const port = makePort()
    const result = sendParts(() => port, messages(), 1000, () => undefined)
    port.reply({ ok: false, error: 'part out of order' })
    expect(await result).toEqual({ ok: false, error: 'part out of order' })
    expect(port.posted).toEqual([{ n: 0 }])
    expect(port.disconnects).toBe(1)
  })

  it('a_disconnect_mid_stream_rejects_with_the_error_text', async () => {
    const port = makePort()
    const result = sendParts(() => port, messages(), 1000, () => 'native host has exited')
    port.reply({ ok: true })
    port.drop()
    await expect(result).rejects.toThrow('native host has exited')
    expect(port.disconnects).toBe(1)
  })

  it('a_disconnect_without_detail_rejects_with_a_default', async () => {
    const port = makePort()
    const result = sendParts(() => port, messages(), 1000, () => undefined)
    port.drop()
    await expect(result).rejects.toThrow('the helper closed the connection')
  })

  it('a_silent_helper_times_out', async () => {
    const port = makePort()
    const result = sendParts(() => port, messages(), 1000, () => undefined)
    const assertion = expect(result).rejects.toThrow('the helper did not answer')
    await vi.advanceTimersByTimeAsync(1001)
    await assertion
    expect(port.disconnects).toBe(1)
  })

  it('a_reply_after_settling_is_ignored', async () => {
    const port = makePort()
    const result = sendParts(() => port, messages(), 1000, () => undefined)
    port.reply({ ok: false, error: 'no' })
    await result
    port.reply({ ok: true })
    port.drop()
    expect(port.posted).toEqual([{ n: 0 }])
    expect(port.disconnects).toBe(1)
  })

  it('a_throwing_postMessage_rejects', async () => {
    const port = makePort()
    port.throwOnPost = true
    await expect(sendParts(() => port, messages(), 1000, () => undefined)).rejects.toThrow('port is closed')
    expect(port.disconnects).toBe(1)
  })

  it('pulls_the_next_message_only_after_the_previous_reply', async () => {
    const port = makePort()
    const pulled: number[] = []
    function* lazy() {
      for (let n = 0; n < 3; n++) {
        pulled.push(n)
        yield { n }
      }
    }
    const result = sendParts(() => port, lazy(), 1000, () => undefined)
    expect(pulled).toEqual([0])
    port.reply({ ok: true })
    expect(pulled).toEqual([0, 1])
    port.reply({ ok: true })
    port.reply({ ok: true })
    expect(pulled).toEqual([0, 1, 2])
    await result
  })
})
