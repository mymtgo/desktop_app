// Runs in the page's MAIN world on endstep.cc. Copies traffic out to the relay.
import { installHooks } from './hook'

installHooks(window, (event) => {
  window.postMessage({ source: 'mymtgo-endstep', event }, location.origin)
})
