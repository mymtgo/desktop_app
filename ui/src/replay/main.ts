import { createApp, h } from 'vue'
import { commands } from '@/bindings'
import { installContextMenuSuppression } from '@/lib/contextMenu'
import { createErrorReporter, installErrorReporting } from '@/lib/errors'
import ReplayPage from './ReplayPage.vue'
import { replayGameId } from './entry'
import 'mana-font/css/mana.css'
import '@/css/app.css'

// The replay window's entry (replays spec section 6): `replay.html?game={id}`
// mounts the replay page only, without the main app's router, live match
// client or query preloads.
const game = replayGameId(window.location.search)
const app = createApp({ render: () => h(ReplayPage, { initialGame: game }) })
installErrorReporting(
  app,
  createErrorReporter((report) => commands.reportUiError(report)),
  () => `replay:${game ?? 'unknown'}`,
)
installContextMenuSuppression()
app.mount('#app')
