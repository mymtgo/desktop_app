import { describe, expect, it } from 'vitest'
import type { PopupStatus } from '../background/validate'
import { startPopup, type PopupElement } from './popup'

class FakeElement implements PopupElement {
  hidden = false
  textContent: string | null = ''
  checked = false
  className = ''
  title = ''
  children: FakeElement[] = []
  classes = new Set<string>()
  listeners: Record<string, Array<() => void>> = {}
  classList = {
    toggle: (name: string, force: boolean) => void (force ? this.classes.add(name) : this.classes.delete(name)),
    remove: (name: string) => void this.classes.delete(name),
  }
  replaceChildren() {
    this.children = []
  }
  append(node: PopupElement) {
    this.children.push(node as FakeElement)
  }
  addEventListener(type: string, listener: () => void) {
    ;(this.listeners[type] ??= []).push(listener)
  }
  fire(type: string) {
    for (const listener of this.listeners[type] ?? []) listener()
  }
}

function status(extra: Partial<PopupStatus> = {}): PopupStatus {
  return {
    captureEnabled: true,
    storageError: null,
    lastStoreError: null,
    recording: 0,
    waiting: [],
    lastDeliveredAt: null,
    dropped: { count: 0, last: null },
    helper: { ok: true, version: '1.0.0', error: null },
    hostAccess: true,
    tabNeedsReload: false,
    ...extra,
  }
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

function setup(reply: (message: object) => unknown, isFirefox = true) {
  const elements = new Map<string, FakeElement>()
  const sent: object[] = []
  const get = (id: string) => {
    let found = elements.get(id)
    if (!found) elements.set(id, (found = new FakeElement()))
    return found
  }
  startPopup({
    element: get,
    createItem: () => new FakeElement(),
    send: async (message) => {
      sent.push(message)
      const result = reply(message)
      if (result instanceof Error) throw result
      return result
    },
    isFirefox,
  })
  return { get, sent }
}

describe('popup', () => {
  it('asks_for_the_status_on_open_and_shows_it', async () => {
    const { get, sent } = setup(() => status({ recording: 2, dropped: { count: 3, last: null } }))
    await flush()
    expect(sent).toEqual([{ popup: 'status' }])
    expect(get('recording').textContent).toBe('2 matches')
    expect(get('dropped').textContent).toBe('3')
    expect(get('dropped-reset').hidden).toBe(false)
    expect(get('access-error').hidden).toBe(true)
    expect(get('reload-hint').hidden).toBe(true)
  })

  it('the_reset_button_sends_a_reset_and_shows_the_new_count', async () => {
    let count = 3
    const { get, sent } = setup((message) => {
      if ((message as { popup: string }).popup === 'resetDropped') count = 0
      return status({ dropped: { count, last: null } })
    })
    await flush()
    get('dropped-reset').fire('click')
    await flush()
    expect(sent).toContainEqual({ popup: 'resetDropped' })
    expect(get('dropped').textContent).toBe('0')
    expect(get('dropped-reset').hidden).toBe(true)
  })

  it('a_failed_switch_change_puts_the_checkbox_back', async () => {
    let fail = false
    const { get, sent } = setup(() => (fail ? { error: 'storage full' } : status({ captureEnabled: true })))
    await flush()
    const box = get('capture-enabled')
    expect(box.checked).toBe(true)
    fail = true
    box.checked = false
    box.fire('change')
    await flush()
    expect(sent).toContainEqual({ popup: 'setEnabled', value: false })
    expect(box.checked).toBe(true)
    expect(get('background-error').hidden).toBe(false)
  })

  it('a_rejected_message_also_puts_the_checkbox_back', async () => {
    let fail = false
    const { get } = setup(() => (fail ? new Error('gone') : status({ captureEnabled: false })))
    await flush()
    const box = get('capture-enabled')
    fail = true
    box.checked = true
    box.fire('change')
    await flush()
    expect(box.checked).toBe(false)
  })

  it('shows_when_the_endstep_permission_is_off', async () => {
    const { get } = setup(() => status({ hostAccess: false }))
    await flush()
    expect(get('access-error').hidden).toBe(false)
    expect(get('access-error').textContent).toContain('add-on')
  })

  it('names_the_site_access_setting_in_chrome', async () => {
    const { get } = setup(() => status({ hostAccess: false }), false)
    await flush()
    expect(get('access-error').textContent).toContain('Site access')
  })

  it('a_failed_change_asks_for_the_status_and_shows_what_the_background_holds', async () => {
    // The change went through, but the reply to it was an error.
    const { get, sent } = setup((message) =>
      (message as { popup: string }).popup === 'setEnabled' ? { error: 'status failed' } : status({ captureEnabled: false }),
    )
    await flush()
    const box = get('capture-enabled')
    box.checked = false
    box.fire('change')
    await flush()
    expect(sent.map((m) => (m as { popup: string }).popup)).toEqual(['status', 'setEnabled', 'status'])
    expect(box.checked).toBe(false)
    expect(get('background-error').hidden).toBe(true)
  })

  it('the_reset_link_is_hidden_when_the_background_does_not_answer', async () => {
    const { get } = setup(() => ({ error: 'down' }))
    await flush()
    expect(get('dropped-reset').hidden).toBe(true)
  })

  it('asks_for_a_reload_when_the_tab_has_no_hook', async () => {
    const { get } = setup(() => status({ tabNeedsReload: true }))
    await flush()
    expect(get('reload-hint').hidden).toBe(false)
  })
})
