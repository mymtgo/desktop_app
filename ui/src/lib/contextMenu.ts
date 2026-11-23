/** Where the WebView's own menu stays: text fields, for copy and paste. */
const EDITABLE = 'input, textarea, [contenteditable]:not([contenteditable="false"])'

/** What `suppressNativeMenu` reads from a `contextmenu` event. */
export interface MenuEvent {
  target: unknown
  defaultPrevented: boolean
  preventDefault(): void
}

/**
 * Hides the WebView's right-click menu ("Open in new tab", "Copy link",
 * which copies a `tauri.localhost` URL). The app's own menus (reka-ui
 * `ContextMenu`) prevent the event first and are left alone.
 */
export function suppressNativeMenu(event: MenuEvent): void {
  if (opensOwnMenu(event)) event.preventDefault()
}

/**
 * Whether a right-click is the window's to handle: not already taken by a
 * menu on an element, and not in a text field (which keeps copy and paste).
 */
export function opensOwnMenu(event: MenuEvent): boolean {
  if (event.defaultPrevented) return false
  const target = event.target as { closest?: (selector: string) => unknown } | null
  return !(typeof target?.closest === 'function' && target.closest(EDITABLE))
}

/**
 * Installs `suppressNativeMenu` on a window. It listens on the window in the
 * bubble phase, so menus on the elements inside have already run.
 */
export function installContextMenuSuppression(target: Pick<Window, 'addEventListener'> = window): void {
  target.addEventListener('contextmenu', suppressNativeMenu)
}
