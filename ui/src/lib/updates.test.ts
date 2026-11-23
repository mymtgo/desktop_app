import { describe, expect, it } from 'vitest'
import type { UpdateStatus } from '@/bindings'
import { checkedAgo, updateLabel } from './updates'

const base: UpdateStatus = { current: '0.1.0', available: null, phase: 'up_to_date', checkedAtMs: null, error: null, active: true, channel: 'edge' }

describe('updateLabel', () => {
  it('says what the updater is doing', () => {
    expect(updateLabel({ ...base, active: false })).toBe('Updates are only checked in the installed app')
    expect(updateLabel({ ...base, phase: 'checking' })).toBe('Checking for updates...')
    expect(updateLabel({ ...base, phase: 'downloading', available: '0.1.1' })).toBe('Downloading v0.1.1...')
    expect(updateLabel({ ...base, phase: 'ready', available: '0.1.1' })).toBe('v0.1.1 is ready to install')
    expect(updateLabel({ ...base, phase: 'error', error: 'offline' })).toBe('Could not check for updates')
    expect(updateLabel(base)).toBe("You're up to date")
  })
})

describe('checkedAgo', () => {
  it('rounds to minutes, then hours', () => {
    const now = 10 * 60 * 60 * 1000
    expect(checkedAgo(null, now)).toBeNull()
    expect(checkedAgo(now - 20_000, now)).toBe('Last checked just now')
    expect(checkedAgo(now - 5 * 60_000, now)).toBe('Last checked 5 min ago')
    expect(checkedAgo(now - 3 * 60 * 60_000, now)).toBe('Last checked 3 h ago')
  })
})
