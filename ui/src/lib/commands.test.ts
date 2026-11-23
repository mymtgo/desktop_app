import { describe, expect, it } from 'vitest'
import { CommandFailed, errorMessage, unwrap } from './commands'

describe('unwrap', () => {
  it('returns the data of an ok result', () => {
    expect(unwrap({ status: 'ok', data: 42 })).toBe(42)
  })

  it('throws the command error of an error result', () => {
    const run = () => unwrap({ status: 'error', error: { kind: 'tracker', message: 'busy' } })
    expect(run).toThrow(CommandFailed)
    expect(run).toThrow('busy')
  })

  it('describes any thrown value', () => {
    expect(errorMessage(new CommandFailed({ kind: 'storage', message: 'disk full' }))).toBe('disk full')
    expect(errorMessage(new Error('boom'))).toBe('boom')
    expect(errorMessage('plain')).toBe('plain')
  })
})
