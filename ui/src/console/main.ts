import { createApp } from 'vue'
import { commands } from '@/bindings'
import { installContextMenuSuppression } from '@/lib/contextMenu'
import { createErrorReporter, installErrorReporting } from '@/lib/errors'
import ConsoleRoot from './ConsoleRoot.vue'
import '@/css/app.css'

// The data console window's entry (`console.html`): mounts the console
// page only, without the main app's router, live match client or query
// preloads.
const app = createApp(ConsoleRoot)
installErrorReporting(
  app,
  createErrorReporter((report) => commands.reportUiError(report)),
  () => 'console',
)
installContextMenuSuppression()
app.mount('#app')
