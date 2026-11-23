import { type Component, createApp, h } from 'vue'
import { commands } from '@/bindings'
import { createErrorReporter, installErrorReporting } from '@/lib/errors'
import DraftNotesPage from './draft/DraftNotesPage.vue'
import GameOverlayPage from './game/GameOverlayPage.vue'
import LeagueOverlayPage from './league/LeagueOverlayPage.vue'
import OverlayRoot from './OverlayRoot.vue'
import { installOverlayMenu } from './overlayMenu'
import { isTransparent, type OverlayView, overlayView } from './views'
import 'mana-font/css/mana.css'
import '@/css/app.css'

// The overlay windows' entry (overlay spec 5.1): one small page per window,
// chosen by `?view=`, without the main app's router or query preloads.
const view = overlayView(window.location.search)
const PAGES: Partial<Record<OverlayView, Component>> = {
  game: GameOverlayPage,
  'draft-notes': DraftNotesPage,
  league: LeagueOverlayPage,
}
const page = view ? PAGES[view] : undefined
// The page body paints the theme background (app.css); a transparent
// window needs none.
if (isTransparent(view)) document.body.classList.add('bg-transparent')
const app = createApp({
  render: () => h(OverlayRoot, { view }, { default: () => (page ? h(page) : null) }),
})
installErrorReporting(
  app,
  createErrorReporter((report) => commands.reportUiError(report)),
  () => `overlay:${view ?? 'unknown'}`,
)
installOverlayMenu()
app.mount('#app')
