import { describe, expect, it } from 'vitest'
import { logFolder } from './paths'

describe('logFolder', () => {
  it('uses the data folder separator', () => {
    expect(logFolder('C:\\Users\\q\\AppData\\Roaming\\com.mymtgo.desktop')).toBe(
      'C:\\Users\\q\\AppData\\Roaming\\com.mymtgo.desktop\\logs',
    )
    expect(logFolder('/home/q/.local/share/com.mymtgo.desktop')).toBe(
      '/home/q/.local/share/com.mymtgo.desktop/logs',
    )
  })

  it('ignores a trailing separator', () => {
    expect(logFolder('/home/q/data/')).toBe('/home/q/data/logs')
  })
})
