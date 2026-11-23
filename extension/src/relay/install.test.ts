import { describe, expect, it, vi } from 'vitest'
import { installRelay, shouldInstallRelay, RELAY_PROBE, type RelayRuntime, type RelayTarget } from './install'

function makeTarget() {
  const listeners: Array<(event: MessageEvent) => void> = []
  const target = {
    addEventListener: (_type: 'message', listener: (event: MessageEvent) => void) => {
      listeners.push(listener)
    },
  } as RelayTarget
  return { target, listeners }
}

function liveRuntime(): RelayRuntime {
  return { id: 'abc', sendMessage: vi.fn(async () => undefined) }
}

function deadRuntime(): RelayRuntime {
  return {
    get id(): string | undefined {
      throw new Error('Extension context invalidated.')
    },
    sendMessage: vi.fn(),
  }
}

describe('shouldInstallRelay', () => {
  it('no_earlier_relay_installs', () => {
    expect(shouldInstallRelay(undefined)).toBe(true)
  })

  it('a_live_earlier_relay_blocks', () => {
    expect(shouldInstallRelay(() => true)).toBe(false)
  })

  it('a_dead_earlier_relay_does_not_block', () => {
    expect(shouldInstallRelay(() => false)).toBe(true)
  })

  it('a_probe_that_throws_does_not_block', () => {
    expect(
      shouldInstallRelay(() => {
        throw new Error('context gone')
      }),
    ).toBe(true)
  })

  it('something_that_is_not_a_probe_does_not_block', () => {
    expect(shouldInstallRelay('yes')).toBe(true)
  })
})

describe('installRelay', () => {
  it('running_twice_in_one_live_context_adds_one_listener', () => {
    const scope: Record<string, unknown> = {}
    const { target, listeners } = makeTarget()
    const runtime = liveRuntime()
    installRelay(scope, target, runtime)
    installRelay(scope, target, runtime)
    expect(listeners).toHaveLength(1)
  })

  it('a_dead_earlier_relay_does_not_block_a_new_one', () => {
    const scope: Record<string, unknown> = {}
    const first = makeTarget()
    installRelay(scope, first.target, deadRuntime())
    const second = makeTarget()
    installRelay(scope, second.target, liveRuntime())
    expect(second.listeners).toHaveLength(1)
    expect((scope[RELAY_PROBE] as () => boolean)()).toBe(true)
  })

  it('the_probe_of_a_dead_relay_answers_false', () => {
    const scope: Record<string, unknown> = {}
    installRelay(scope, makeTarget().target, deadRuntime())
    expect((scope[RELAY_PROBE] as () => boolean)()).toBe(false)
  })

  it('the_listener_forwards_hook_events', () => {
    const scope: Record<string, unknown> = {}
    const { target, listeners } = makeTarget()
    const runtime = liveRuntime()
    installRelay(scope, target, runtime)
    listeners[0]({
      source: target,
      data: { source: 'mymtgo-endstep', event: { kind: 'page_start' } },
    } as unknown as MessageEvent)
    expect(runtime.sendMessage).toHaveBeenCalledWith({ endstep: { kind: 'page_start' } })
  })
})
