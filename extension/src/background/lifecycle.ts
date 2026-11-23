// What the background does when the browser installs, updates or loads it. The chrome calls are
// passed in, so tests can run these without a browser.
export interface LifecycleDeps {
  // Runs the relay again in open endstep.cc tabs.
  reinject: () => Promise<unknown>
  startUp: () => Promise<unknown>
  ensureAlarm: () => Promise<unknown>
}

// Starts a step without waiting for it. A failure is logged, never thrown.
function run(step: () => Promise<unknown>): void {
  step().catch((error: unknown) => console.warn('mymtgo-endstep: startup step failed', error))
}

// Every time the worker (or Firefox's background script) loads. A Chrome disable then enable
// raises no onInstalled, so the relay is re-run here (it guards against a double install).
export function onWorkerLoad(deps: LifecycleDeps): void {
  run(deps.ensureAlarm)
  run(deps.reinject)
  run(deps.startUp)
}

// install and update (an unpacked reload reports update) leave open Endstep tabs with a dead relay.
// browser_update (Firefox), chrome_update (Chrome) and shared_module_update do not.
// startUp can take seconds and frames are dropped while no relay is live, so inject at once.
export function onInstalled(details: { reason: string }, deps: Pick<LifecycleDeps, 'reinject' | 'startUp'>): void {
  if (details.reason === 'install' || details.reason === 'update') run(deps.reinject)
  run(deps.startUp)
}
