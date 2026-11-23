import { VueQueryPlugin } from '@tanstack/vue-query'
import { createApp } from 'vue'
import App from './App.vue'
import { commands } from './bindings'
import { installContextMenuSuppression } from './lib/contextMenu'
import { createErrorReporter, installErrorReporting } from './lib/errors'
import { startLiveEvents } from './lib/liveEvents'
import { queryClient } from './queryClient'
import { router } from './router'
import 'mana-font/css/mana.css'
import './css/app.css'

// First, so an error while starting is reported too.
const app = createApp(App)
installErrorReporting(
  app,
  createErrorReporter((report) => commands.reportUiError(report)),
  () => router.currentRoute.value.fullPath,
)
installContextMenuSuppression()
await startLiveEvents()
app.use(router).use(VueQueryPlugin, { queryClient }).mount('#app')
