import { describe, expect, it } from 'vitest'
import { actionFor } from './keys'

const plain = { ctrlKey: false, metaKey: false, shiftKey: false }

describe('actionFor', () => {
  it('ctrl or cmd and enter runs, in the editor too', () => {
    expect(actionFor({ ...plain, key: 'Enter', ctrlKey: true }, true)).toBe('run')
    expect(actionFor({ ...plain, key: 'Enter', metaKey: true }, false)).toBe('run')
    expect(actionFor({ ...plain, key: 'Enter' }, true)).toBeNull()
  })

  it('esc cancels anywhere', () => {
    expect(actionFor({ ...plain, key: 'Escape' }, true)).toBe('cancel')
    expect(actionFor({ ...plain, key: 'Escape' }, false)).toBe('cancel')
  })

  it('single letters act only outside the editor', () => {
    expect(actionFor({ ...plain, key: 's' }, true)).toBeNull()
    expect(actionFor({ ...plain, key: 's' }, false)).toBe('save')
    expect(actionFor({ ...plain, key: 'e' }, false)).toBe('export')
    expect(actionFor({ ...plain, key: 'c' }, false)).toBe('chart')
    expect(actionFor({ ...plain, key: 'o' }, false)).toBe('folder')
    expect(actionFor({ ...plain, key: 's', ctrlKey: true }, false)).toBeNull()
    expect(actionFor({ ...plain, key: 'x' }, false)).toBeNull()
  })

  it('alt and a letter is never a single-letter action', () => {
    for (const key of ['s', 'e', 'c', 'o']) {
      expect(actionFor({ ...plain, key, altKey: true }, false)).toBeNull()
    }
  })

  it('tab cycles panes; from the editor only ctrl and tab does', () => {
    expect(actionFor({ ...plain, key: 'Tab' }, false)).toBe('nextPane')
    expect(actionFor({ ...plain, key: 'Tab', shiftKey: true }, false)).toBe('prevPane')
    expect(actionFor({ ...plain, key: 'Tab' }, true)).toBeNull()
    expect(actionFor({ ...plain, key: 'Tab', ctrlKey: true }, true)).toBe('nextPane')
    expect(actionFor({ ...plain, key: 'Tab', ctrlKey: true, shiftKey: true }, true)).toBe('prevPane')
  })
})
