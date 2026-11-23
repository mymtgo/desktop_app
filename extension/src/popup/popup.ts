// Popup logic: shows the background's status and sends the popup's requests to it. The page
// supplies the elements and the message call, so tests can run it without a browser.
import type { PopupStatus } from '../background/validate'

export interface PopupElement {
  hidden: boolean
  textContent: string | null
  checked: boolean
  className: string
  title: string
  classList: { toggle(name: string, force: boolean): unknown; remove(name: string): unknown }
  replaceChildren(): void
  append(node: PopupElement): void
  addEventListener(type: string, listener: () => void): void
}

export interface PopupDeps {
  element: (id: string) => PopupElement
  createItem: () => PopupElement
  send: (message: object) => Promise<unknown>
  isFirefox: boolean
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString()
}

// Time only when it is today, else short date and time (no seconds).
function formatShort(ms: number): string {
  const date = new Date(ms)
  const time = date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
  if (date.toDateString() === new Date().toDateString()) return time
  return `${date.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}, ${time}`
}

function describeSkip(kind: 'match' | 'session', reason: 'bot' | 'spectator' | 'empty'): string {
  if (reason === 'bot') return 'a match against the AI'
  if (reason === 'spectator') return 'a match you watched'
  return kind === 'session' ? 'an empty session' : 'an empty match'
}

export function startPopup(deps: PopupDeps): void {
  const { element } = deps
  const captureEnabled = element('capture-enabled')
  // The switch as the background last reported it, so a failed change can put it back.
  let shownEnabled = captureEnabled.checked

  function render(status: PopupStatus) {
    element('background-error').hidden = true
    element('storage-error').hidden = status.storageError === null
    element('firefox-hint').hidden = !deps.isFirefox
    const access = element('access-error')
    access.hidden = status.hostAccess
    access.textContent = deps.isFirefox
      ? "Endstep access is off: allow it in the add-on's permissions."
      : "Endstep access is limited: set the extension's Site access to allow endstep.cc."
    element('reload-hint').hidden = !status.tabNeedsReload
    const storeError = element('store-error')
    storeError.hidden = status.lastStoreError === null
    storeError.textContent = status.lastStoreError
      ? `Could not store a capture (${formatTime(status.lastStoreError.at)}): ${status.lastStoreError.message}`
      : ''
    shownEnabled = status.captureEnabled
    captureEnabled.checked = shownEnabled

    const helper = element('helper')
    helper.textContent = status.helper.ok ? 'Connected to MyMTGO' : 'MyMTGO helper not found'
    helper.title = status.helper.error ?? ''
    element('helper-dot').className = status.helper.ok ? 'dot ok' : 'dot error'
    element('helper-version').textContent = status.helper.ok ? (status.helper.version ?? '') : ''

    const recording = element('recording')
    recording.textContent = status.recording === 1 ? '1 match' : status.recording > 0 ? `${status.recording} matches` : '0'
    // The pulsing dot shows only while something is being recorded.
    recording.classList.toggle('recording', status.recording > 0)
    element('waiting-count').textContent = String(status.waiting.length)
    const list = element('waiting')
    list.replaceChildren()
    for (const unit of status.waiting) {
      const item = deps.createItem()
      item.textContent = `${unit.kind} from ${formatTime(unit.startedAt)}: ${unit.error ?? 'not sent yet'}`
      list.append(item)
    }
    element('last-sent').textContent = status.lastDeliveredAt === null ? 'never' : formatShort(status.lastDeliveredAt)
    const { count, last } = status.dropped
    element('dropped').textContent = String(count)
    element('dropped-reset').hidden = count === 0
    element('dropped-row').title = last === null ? '' : `Last skipped: ${describeSkip(last.kind, last.reason)}, ${formatShort(last.at)}`
  }

  // Replaces every status field with a dash and shows why the background gave no status.
  function renderError(message: string) {
    const shown = element('background-error')
    shown.hidden = false
    shown.textContent = `The extension's background did not answer: ${message}`
    element('dropped-reset').hidden = true
    for (const id of ['recording', 'waiting-count', 'last-sent', 'dropped']) element(id).textContent = '-'
    element('recording').classList.remove('recording')
    element('helper').textContent = 'No answer from the background'
    element('helper-dot').className = 'dot error'
    element('helper-version').textContent = ''
    element('waiting').replaceChildren()
  }

  // After a failed change the switch may or may not have changed in the background, so the popup
  // asks for the status again. When even that gets no answer, the switch goes back to the last known state.
  function fail(message: { popup: string; value?: boolean }, text: string) {
    renderError(text)
    if (message.popup === 'status') captureEnabled.checked = shownEnabled
    else void request({ popup: 'status' })
  }

  async function request(message: { popup: string; value?: boolean }) {
    try {
      const reply = (await deps.send(message)) as PopupStatus | { error: string } | undefined
      if (reply && 'helper' in reply) render(reply)
      else if (reply && 'error' in reply) fail(message, reply.error)
      else fail(message, 'no reply')
    } catch (error) {
      fail(message, error instanceof Error ? error.message : String(error))
    }
  }

  captureEnabled.addEventListener('change', () => void request({ popup: 'setEnabled', value: captureEnabled.checked }))
  element('dropped-reset').addEventListener('click', () => void request({ popup: 'resetDropped' }))
  void request({ popup: 'status' })
}
