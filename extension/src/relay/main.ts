// Runs in the isolated world. Forwards the page hook's events to the background.
import { installRelay, type RelayRuntime } from './install'

installRelay(globalThis as unknown as Record<string, unknown>, window, chrome.runtime as unknown as RelayRuntime)
