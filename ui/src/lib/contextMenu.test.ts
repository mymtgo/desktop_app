import { describe, expect, it, vi } from 'vitest'
import { suppressNativeMenu } from './contextMenu'

function element(editable: boolean) {
  return { closest: (selector: string) => (editable && selector.includes('input') ? {} : null) }
}

function rightClick(target: unknown, defaultPrevented = false) {
  return { target, defaultPrevented, preventDefault: vi.fn() }
}

describe('suppressNativeMenu', () => {
  it('hides the WebView menu on links and plain elements', () => {
    const event = rightClick(element(false))
    suppressNativeMenu(event)
    expect(event.preventDefault).toHaveBeenCalledOnce()
  })

  it('keeps the menu in text fields, for copy and paste', () => {
    const event = rightClick(element(true))
    suppressNativeMenu(event)
    expect(event.preventDefault).not.toHaveBeenCalled()
  })

  it('leaves events a custom menu already handled alone', () => {
    const event = rightClick(element(false), true)
    suppressNativeMenu(event)
    expect(event.preventDefault).not.toHaveBeenCalled()
  })

  it('hides the menu when the target is not an element', () => {
    const event = rightClick(null)
    suppressNativeMenu(event)
    expect(event.preventDefault).toHaveBeenCalledOnce()
  })
})
