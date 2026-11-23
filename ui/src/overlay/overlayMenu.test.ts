import { describe, expect, it, vi } from 'vitest'
import { overlayMenu, overlayMenuHandler } from './overlayMenu'

const minimize = vi.fn()

vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ minimize }) }))

vi.mock('@tauri-apps/api/menu', () => {
  class MenuItem {
    constructor(readonly options: { id: string; text: string; action: () => void }) {}
    static async new(options: { id: string; text: string; action: () => void }) {
      return new MenuItem(options)
    }
  }
  class Menu {
    constructor(readonly options: { items: unknown[] }) {}
    static async new(options: { items: unknown[] }) {
      return new Menu(options)
    }
  }
  return { Menu, MenuItem }
})

function element(editable: boolean) {
  return { closest: (selector: string) => (editable && selector.includes('textarea') ? {} : null) }
}

function rightClick(target: unknown, defaultPrevented = false) {
  return { target, defaultPrevented, preventDefault: vi.fn() }
}

describe('overlayMenuHandler', () => {
  it('opens the overlay menu instead of the WebView menu', () => {
    const show = vi.fn()
    const event = rightClick(element(false))
    overlayMenuHandler(show)(event)
    expect(event.preventDefault).toHaveBeenCalledOnce()
    expect(show).toHaveBeenCalledOnce()
  })

  it('keeps the WebView menu in text fields, for copy and paste', () => {
    const show = vi.fn()
    const event = rightClick(element(true))
    overlayMenuHandler(show)(event)
    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(show).not.toHaveBeenCalled()
  })

  it('leaves events a custom menu already handled alone', () => {
    const show = vi.fn()
    const event = rightClick(element(false), true)
    overlayMenuHandler(show)(event)
    expect(show).not.toHaveBeenCalled()
  })
})

describe('overlayMenu', () => {
  // Tauri 2.12.1 drops an inline item's action as soon as the menu is built
  // (the Rust item is freed, and its click handler with it), so the menu is
  // built from a MenuItem that lives as long as the window.
  it('builds Minimise as its own MenuItem, which minimises the window', async () => {
    const { MenuItem } = await import('@tauri-apps/api/menu')
    const menu = (await overlayMenu()) as unknown as { options: { items: unknown[] } }
    const [item] = menu.options.items
    expect(item).toBeInstanceOf(MenuItem)
    const { options } = item as { options: { text: string; action: () => void } }
    expect(options.text).toBe('Minimise')
    options.action()
    expect(minimize).toHaveBeenCalledOnce()
  })
})
