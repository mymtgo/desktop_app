import { Menu, MenuItem } from '@tauri-apps/api/menu'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { type MenuEvent, opensOwnMenu } from '@/lib/contextMenu'

/**
 * The overlay windows' right-click (#510): a native menu with "Minimise"
 * in place of the WebView's own, which still shows in text fields.
 */
export function overlayMenuHandler(show: () => void): (event: MenuEvent) => void {
  return (event) => {
    if (!opensOwnMenu(event)) return
    event.preventDefault()
    show()
  }
}

/** The menu, made once: it lives on the Rust side until the window closes. */
let menu: Promise<Menu> | undefined

/**
 * The item is made on its own, not inline in `Menu.new`: Tauri 2.12.1 frees
 * an inline item once the menu is built, and its click handler with it, so
 * "Minimise" did nothing. A `MenuItem` is held until the window closes.
 */
export function overlayMenu(): Promise<Menu> {
  menu ??= MenuItem.new({
    id: 'minimise',
    text: 'Minimise',
    action: () => void getCurrentWindow().minimize(),
  }).then((item) => Menu.new({ items: [item] }))
  return menu
}

/** Pops the menu up at the mouse. A native menu is not clipped by the small window. */
async function showOverlayMenu(): Promise<void> {
  await (await overlayMenu()).popup()
}

/** Installs the menu on the overlay window, in the bubble phase like `installContextMenuSuppression`. */
export function installOverlayMenu(): void {
  window.addEventListener('contextmenu', overlayMenuHandler(() => void showOverlayMenu()))
}
