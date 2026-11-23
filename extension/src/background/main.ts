// Background script: Chrome service worker, Firefox background script. Chrome wiring only; the
// logic is in engine.ts.
// Every unit action goes to IndexedDB as it happens (a service worker stops after about 30 s idle,
// spec 4.5); closed units are sent to the native helper and removed only once it confirms.
import { UnitTracker } from '../capture/units'
import { errorText, type DropRecord, type HostReply } from './delivery'
import { Engine } from './engine'
import { sendParts } from './port'
import { reinjectRelay, type ReinjectApi } from './reinject'
import { ensureAlarm } from './alarm'
import { DroppedCounter } from './dropped'
import { hasHostAccess, watchGrants } from './access'
import { onInstalled, onWorkerLoad } from './lifecycle'
import { activeTabNeedsReload, type TabHookApi } from './tabHook'
import { IdbStore } from './idb'
import type { CaptureStore } from './store'
import {
  parseHookMessage,
  parsePopupMessage,
  type PopupRequest,
  type PopupStatus,
  type SenderInfo,
} from './validate'

const HOST = 'com.mymtgo.endstep'
const PING_TIMEOUT_MS = 5000
const PART_REPLY_TIMEOUT_MS = 60_000
const PING_ALARM = 'mymtgo-ping'
const PING_EVERY_MINUTES = 30
const BROWSER = chrome.runtime.getURL('').startsWith('moz-extension:') ? 'firefox' : 'chrome'
const EXTENSION_VERSION = chrome.runtime.getManifest().version

interface Settings {
  captureEnabled: boolean
}

async function loadSettings(): Promise<Settings> {
  const stored = await chrome.storage.local.get(['captureEnabled'])
  return { captureEnabled: stored.captureEnabled !== false }
}

// The keep-bots setting is gone: bot matches are never kept. Clear the key an older version stored.
chrome.storage.local.remove('keepBots').catch(() => {})

let settings: Promise<Settings> = loadSettings()
chrome.storage.onChanged.addListener((_changes, area) => {
  if (area === 'local') settings = loadSettings()
})

let storageError: string | null = null
const storeReady: Promise<CaptureStore | null> = IdbStore.open().catch((error: unknown) => {
  storageError = errorText(error)
  return null
})

function sendToHelper(msg: object): Promise<HostReply> {
  return chrome.runtime.sendNativeMessage(HOST, msg) as Promise<HostReply>
}

function sendPartsToHelper(messages: Iterable<object>): Promise<HostReply> {
  return sendParts(
    () => chrome.runtime.connectNative(HOST),
    messages,
    PART_REPLY_TIMEOUT_MS,
    () => chrome.runtime.lastError?.message,
  )
}

const droppedCounter = new DroppedCounter(chrome.storage.local, () => Date.now())
const recordDrop = (drop: DropRecord) => droppedCounter.record(drop)

function readDropped(stored: Record<string, unknown>): PopupStatus['dropped'] {
  const count = typeof stored.droppedCount === 'number' ? stored.droppedCount : 0
  const last = stored.lastDrop as { kind?: unknown; reason?: unknown; at?: unknown } | undefined
  const valid =
    typeof last === 'object' &&
    last !== null &&
    (last.kind === 'match' || last.kind === 'session') &&
    (last.reason === 'bot' || last.reason === 'spectator' || last.reason === 'empty') &&
    typeof last.at === 'number'
  return {
    count,
    last: valid ? { kind: last.kind as 'match' | 'session', reason: last.reason as DropRecord['reason'], at: last.at as number } : null,
  }
}

const engine = new Engine({
  store: storeReady,
  newTracker: () => new UnitTracker(() => crypto.randomUUID()),
  send: sendToHelper,
  sendParts: sendPartsToHelper,
  captureEnabled: async () => (await settings).captureEnabled,
  version: { extension: EXTENSION_VERSION, browser: BROWSER },
  now: () => Date.now(),
  recordDrop,
  // Kept in storage, not memory, so the popup still shows it after the worker restarts.
  recordStoreError: (error) => chrome.storage.local.set({ lastStoreError: error }),
  clearStoreError: () => chrome.storage.local.remove('lastStoreError'),
  recordDelivered: (at) => chrome.storage.local.set({ lastDeliveredAt: at }),
  ping: async () => void (await pingHelper()),
})

function readStoreError(stored: Record<string, unknown>): PopupStatus['lastStoreError'] {
  const last = stored.lastStoreError as { message?: unknown; at?: unknown } | undefined
  if (typeof last !== 'object' || last === null) return null
  if (typeof last.message !== 'string' || typeof last.at !== 'number') return null
  return { message: last.message, at: last.at }
}

async function pingHelper(): Promise<PopupStatus['helper']> {
  let timer: ReturnType<typeof setTimeout> | undefined
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error('the helper did not answer')), PING_TIMEOUT_MS)
  })
  try {
    const reply = (await Promise.race([sendToHelper({ type: 'ping', browser: BROWSER, extension: EXTENSION_VERSION }), timeout])) as HostReply | undefined
    if (reply?.ok === true) return { ok: true, version: reply.version ?? null, error: null }
    return { ok: false, version: null, error: reply?.error ?? 'no answer' }
  } catch (error) {
    return { ok: false, version: null, error: errorText(error) }
  } finally {
    clearTimeout(timer)
  }
}

const tabHookApi: TabHookApi = {
  activeEndstepTab: async (urlPattern) => {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true, url: urlPattern })
    return typeof tab?.id === 'number' ? tab.id : null
  },
  hasHook: async (tabId, marker) => {
    // Runs in the page's own world, where the hook sets its marker. Reads one property and changes nothing.
    const [result] = await chrome.scripting.executeScript({
      target: { tabId },
      world: 'MAIN',
      injectImmediately: true,
      func: (name: string) => Object.prototype.hasOwnProperty.call(window, name),
      args: [marker],
    })
    return result?.result === true
  },
}

async function status(): Promise<PopupStatus> {
  const store = await storeReady
  const current = await settings
  const closed = store ? await store.closedUnits() : []
  const recording = store ? (await store.openUnits('match')).length : 0
  const stored = await chrome.storage.local.get(['lastDeliveredAt', 'droppedCount', 'lastDrop', 'lastStoreError'])
  // The three checks are independent and each may be slow, so they run together.
  const [helper, hostAccess, tabNeedsReload] = await Promise.all([
    pingHelper(),
    hasHostAccess(chrome.permissions),
    activeTabNeedsReload(tabHookApi),
  ])
  const { lastDeliveredAt } = stored
  return {
    captureEnabled: current.captureEnabled,
    storageError,
    lastStoreError: readStoreError(stored),
    recording,
    waiting: closed.map((u) => ({ unitId: u.unitId, kind: u.unitKind, startedAt: u.startedAt, error: u.error ?? null })),
    lastDeliveredAt: typeof lastDeliveredAt === 'number' ? lastDeliveredAt : null,
    dropped: readDropped(stored),
    helper,
    hostAccess,
    tabNeedsReload,
  }
}

async function answerPopup(request: PopupRequest): Promise<PopupStatus> {
  if (request.popup === 'resetDropped') {
    await droppedCounter.reset()
  }
  if (request.popup === 'setEnabled') {
    await chrome.storage.local.set({ captureEnabled: request.value })
    settings = loadSettings()
    if (!request.value) await engine.stopCapture()
  }
  return status()
}

chrome.runtime.onMessage.addListener((message: unknown, sender, sendResponse) => {
  const info: SenderInfo = { url: sender.url, tabUrl: sender.tab?.url, hasTab: sender.tab !== undefined }
  if (sender.tab) {
    const tabId = sender.tab.id
    if (typeof tabId !== 'number') return
    const event = parseHookMessage(message, info)
    if (event) void engine.handleHookEvent(tabId, event, Date.now())
    return
  }
  const request = parsePopupMessage(message, info, chrome.runtime.getURL(''))
  if (!request) return
  answerPopup(request).then(sendResponse, (error: unknown) => sendResponse({ error: errorText(error) }))
  return true
})

chrome.tabs.onRemoved.addListener((tabId) => void engine.handleTabClosed(tabId))

// A repeating alarm survives the worker stopping and wakes it, so the helper hears from the
// extension about every 30 minutes while the browser runs. Created on load only when missing.
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === PING_ALARM) void pingHelper()
})

chrome.runtime.onStartup.addListener(() => void engine.startUp())

const reinjectApi: ReinjectApi = {
  queryTabs: async (urlPattern) => {
    const tabs = await chrome.tabs.query({ url: urlPattern })
    return tabs.flatMap((tab) => (typeof tab.id === 'number' ? [tab.id] : []))
  },
  runFile: async (tabId, file) => {
    await chrome.scripting.executeScript({ target: { tabId }, files: [file] })
  },
}

const reinject = () => reinjectRelay(reinjectApi)
const startUp = () => engine.startUp()

chrome.runtime.onInstalled.addListener((details) => onInstalled(details, { reinject, startUp }))
watchGrants(chrome.permissions.onAdded, reinject)
onWorkerLoad({
  reinject,
  startUp,
  ensureAlarm: () => ensureAlarm(chrome.alarms, PING_ALARM, PING_EVERY_MINUTES),
})
