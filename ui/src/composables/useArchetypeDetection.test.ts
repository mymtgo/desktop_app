import { beforeEach, describe, expect, it, vi } from 'vitest'

const state = vi.hoisted(() => ({
  toasts: [] as { type: string; title: string; message?: string }[],
  detected: 0,
  // The `matchDetected` listener while a run listens, and whether it stopped.
  onDetected: null as ((event: { payload: { token: string } }) => void) | null,
  stopped: 0,
  // Called inside the command: the worker's events during the run.
  during: async () => {},
}))

vi.mock('@tanstack/vue-query', () => ({ useQueryClient: () => ({ invalidateQueries: async () => {} }) }))
vi.mock('@/composables/useToast', () => ({
  useToast: () => ({ add: (toast: { type: string; title: string; message?: string }) => state.toasts.push(toast) }),
}))
vi.mock('@/bindings', () => ({
  commands: {
    detectArchetypes: async () => {
      await state.during()
      return { status: 'ok', data: state.detected }
    },
  },
  events: {
    matchDetected: {
      listen: async (handler: (event: { payload: { token: string } }) => void) => {
        state.onDetected = handler
        return () => {
          state.onDetected = null
          state.stopped += 1
        }
      },
    },
  },
}))

const { useArchetypeDetection } = await import('./useArchetypeDetection')

beforeEach(() => {
  state.toasts = []
  state.onDetected = null
  state.stopped = 0
  state.during = async () => {}
})

describe('detecting matches by token', () => {
  it('says how many were detected when several were asked for', async () => {
    state.detected = 2
    await useArchetypeDetection().detectMatches(['q-1', 'q-2', 'q-3'])
    expect(state.toasts).toEqual([{ type: 'success', title: 'Archetypes detected', message: 'Detected archetypes for 2 matches' }])
  })

  it('holds the tokens being detected until the answer comes', async () => {
    state.detected = 2
    const detection = useArchetypeDetection()
    const running = detection.detectMatches(['q-1', 'q-2'])
    expect(detection.detectingTokens.value).toEqual(['q-1', 'q-2'])
    await running
    expect(detection.detectingTokens.value).toEqual([])
  })

  it('lets each match go as the worker writes it, and stops listening after', async () => {
    state.detected = 2
    const detection = useArchetypeDetection()
    const seen: string[][] = []
    state.during = async () => {
      state.onDetected?.({ payload: { token: 'q-2' } })
      await new Promise((resolve) => setTimeout(resolve, 0))
      seen.push([...detection.detectingTokens.value])
      state.onDetected?.({ payload: { token: 'q-other' } })
      await new Promise((resolve) => setTimeout(resolve, 0))
      seen.push([...detection.detectingTokens.value])
    }
    await detection.detectMatches(['q-1', 'q-2'])
    expect(seen).toEqual([['q-1'], ['q-1']])
    expect(detection.detectingTokens.value).toEqual([])
    expect(state.stopped).toBe(1)
  })

  it('stays quiet for one match, whose row shows the answer', async () => {
    state.detected = 1
    await useArchetypeDetection().detectMatches(['q-1'])
    expect(state.toasts).toEqual([])
  })

  it("says when one match's archetype could not be determined (0.x toast)", async () => {
    state.detected = 0
    await useArchetypeDetection().detectMatches(['q-1'])
    expect(state.toasts).toEqual([
      { type: 'error', title: 'Detection failed', message: "Could not determine the opponent's archetype for this match." },
    ])
  })

  it('says when none of several matches could be determined', async () => {
    state.detected = 0
    await useArchetypeDetection().detectMatches(['q-1', 'q-2'])
    expect(state.toasts).toEqual([
      { type: 'error', title: 'Detection failed', message: "Could not determine the opponents' archetypes for these matches." },
    ])
  })
})
