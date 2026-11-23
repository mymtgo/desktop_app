import type { HostReply } from './delivery'

// The part of chrome.runtime.Port that sendParts uses, so tests can fake it.
export interface PortLike {
  postMessage: (message: object) => void
  disconnect: () => void
  onMessage: { addListener: (listener: (reply: HostReply) => void) => void }
  onDisconnect: { addListener: (listener: () => void) => void }
}

// Sends the messages one by one on a single native port, pulling the next message only when the
// previous reply arrives. Resolves with the first failed reply, or the reply to the last message.
// An open port keeps a Chrome service worker alive, so the port is closed as soon as this settles.
// `lastError` reads the browser's disconnect reason.
export function sendParts(
  connect: () => PortLike,
  messages: Iterable<object>,
  timeoutMs: number,
  lastError: () => string | undefined,
): Promise<HostReply> {
  return new Promise<HostReply>((resolve, reject) => {
    const port = connect()
    const source = messages[Symbol.iterator]()
    let timer: ReturnType<typeof setTimeout> | undefined
    let settled = false
    const finish = (settle: () => void) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      port.disconnect()
      settle()
    }
    const sendNext = (previous?: HostReply) => {
      clearTimeout(timer)
      timer = setTimeout(() => finish(() => reject(new Error('the helper did not answer'))), timeoutMs)
      try {
        const next = source.next()
        if (next.done) {
          finish(() => (previous ? resolve(previous) : reject(new Error('nothing to send'))))
          return
        }
        port.postMessage(next.value)
      } catch (error) {
        finish(() => reject(error))
      }
    }
    port.onMessage.addListener((reply) => {
      if (settled) return
      if (reply?.ok !== true) {
        finish(() => resolve(reply))
        return
      }
      sendNext(reply)
    })
    port.onDisconnect.addListener(() => {
      const detail = lastError()
      finish(() => reject(new Error(detail ?? 'the helper closed the connection')))
    })
    sendNext()
  })
}
