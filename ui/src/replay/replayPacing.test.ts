import { afterAll, describe, expect, test, vi } from 'vitest'
import { defineComponent, shallowRef, watch } from 'vue'
import { mountFake } from './replayTestHarness'
import type { ReplayFrame } from './types'

// Spec ruling 4 (owner: no stuttering). The playhead runs on
// requestAnimationFrame through @vueuse/core, which only drives it when a
// window exists, so the window is this global (where vitest's fake timers
// put their requestAnimationFrame) before the composables load.
vi.stubGlobal('window', globalThis)
vi.stubGlobal('document', {})
// Node has no animation frames; these stand in until the fake timers replace them.
vi.stubGlobal('requestAnimationFrame', () => 0)
vi.stubGlobal('cancelAnimationFrame', () => {})
const { REPLAY_SPEEDS, useReplayPlayback } = await import('./useReplayPlayback')
const { useReplayPlayhead } = await import('./useReplayPlayhead')
const { deriveTurns, frameAtPosition, timelinePositions, timestampMs } = await import('./replayTimeline')

afterAll(() => {
  vi.unstubAllGlobals()
})

// V4's pair: the frames the store keeps without and with repeats dropped.
const fixtures = import.meta.glob<ReplayFrame[]>('./__fixtures__/repeats-*.json', { eager: true, import: 'default' })
const all = fixtures['./__fixtures__/repeats-all.json']
const dropped = fixtures['./__fixtures__/repeats-dropped.json']

const MIN_FRAME_MS = 250

function same(a: ReplayFrame, b: ReplayFrame): boolean {
  return JSON.stringify(a.content) === JSON.stringify(b.content)
}

/** The indexes of `frames` that dropping repeats keeps: each run's first frame, and the last frame. */
function keptIndexes(frames: ReplayFrame[]): number[] {
  return frames.flatMap((frame, i) => (i === 0 || i === frames.length - 1 || !same(frame, frames[i - 1]) ? [i] : []))
}

/** A dropped repeat in the same second as the frame before it: held for the 250 ms floor with nothing moving. */
function isSameSecondRepeat(frames: ReplayFrame[], i: number): boolean {
  return i > 0 && same(frames[i], frames[i - 1]) && frames[i].timestamp === frames[i - 1].timestamp
}

type Run = { shownAt: number[]; elapsed: number[]; ended: boolean }

/** Plays a game from the start at `speed` on fake timers; when each frame first shows and every published game time. */
async function play(frames: ReplayFrame[], speed: number): Promise<Run> {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'requestAnimationFrame', 'cancelAnimationFrame', 'performance', 'Date'] })
  const shownAt: number[] = [0]
  const elapsed: number[] = []
  let playback: ReturnType<typeof useReplayPlayback> | null = null
  const start = performance.now()
  const mounted = mountFake(
    defineComponent({
      setup() {
        const list = shallowRef(frames)
        const turns = shallowRef(deriveTurns(frames, []))
        playback = useReplayPlayback(list, turns)
        const playhead = useReplayPlayhead(list, playback.current, playback.playing, playback.speed)
        watch(
          playback.current,
          (index) => {
            shownAt[index] ??= performance.now() - start
          },
          { flush: 'sync' },
        )
        watch(playhead.elapsed, (value) => elapsed.push(value), { flush: 'sync', immediate: true })
        return () => null
      },
    }),
  )
  playback!.setSpeed(speed)
  playback!.toggle()
  // Async, so Vue's queued watchers run between timers as in the window.
  await vi.advanceTimersByTimeAsync(120_000)
  const ended = !playback!.playing.value && playback!.current.value === frames.length - 1
  mounted.unmount()
  vi.useRealTimers()
  expect(mounted.warnings).toEqual([])
  return { shownAt, elapsed, ended }
}

describe('replay pacing', () => {
  test('dropping_repeats_keeps_playback_pacing_and_scrubber_positions', async () => {
    const kept = keptIndexes(all)
    // The pair is what the rule gives, and it holds same-second repeats to test.
    expect(kept.map((i) => all[i])).toEqual(dropped)
    expect(all.some((_, i) => isSameSecondRepeat(all, i))).toBe(true)

    // The same game with its same-second repeats removed and its later-second ones kept.
    const noSameSecond = all.filter((_, i) => !isSameSecondRepeat(all, i))
    expect(keptIndexes(noSameSecond).map((i) => noSameSecond[i])).toEqual(dropped)

    for (const speed of REPLAY_SPEEDS) {
      const full = await play(all, speed)
      const deduplicated = await play(dropped, speed)
      const plain = await play(noSameSecond, speed)
      expect([full.ended, deduplicated.ended, plain.ended]).toEqual([true, true, true])

      // (a) Each kept frame first shows at the same playback clock, sooner by
      // exactly 250 ms / speed per removed same-second repeat before it, never later.
      kept.forEach((i, j) => {
        let removed = 0
        for (let r = 0; r < i; r++) {
          if (!kept.includes(r) && isSameSecondRepeat(all, r)) removed++
        }
        const sooner = full.shownAt[i] - deduplicated.shownAt[j]
        // Timers run in whole milliseconds (the fake ones as the webview's),
        // so each removed hold of 250 ms / speed counts as its whole part.
        expect(sooner, `speed ${speed}, frame ${j}`).toBe(removed * Math.trunc(MIN_FRAME_MS / speed))
        expect(Math.abs(sooner - (removed * MIN_FRAME_MS) / speed)).toBeLessThan(removed + 1e-9)
        expect(sooner).toBeGreaterThanOrEqual(0)
      })
      // Identically when the game has no same-second repeat.
      keptIndexes(noSameSecond).forEach((i, j) => {
        expect(plain.shownAt[i], `speed ${speed}, frame ${j}`).toBe(deduplicated.shownAt[j])
      })

      // (d) The game time shown never goes back.
      for (const run of [full, deduplicated, plain]) {
        run.elapsed.forEach((value, i) => {
          if (i > 0) expect(value).toBeGreaterThanOrEqual(run.elapsed[i - 1])
        })
      }
      expect(deduplicated.elapsed.at(-1)).toBe(timestampMs(dropped.at(-1)!.timestamp) - timestampMs(dropped[0].timestamp))
    }

    // (b) Every kept frame keeps its scrubber position, and both end at 100.
    const fullPositions = timelinePositions(all)
    const droppedPositions = timelinePositions(dropped)
    kept.forEach((i, j) => expect(droppedPositions[j]).toBe(fullPositions[i]))
    expect(fullPositions.at(-1)).toBe(100)
    expect(droppedPositions.at(-1)).toBe(100)

    // (c) Every 0.1% of the scrubber shows the same board in both runs.
    for (let step = 0; step <= 1000; step++) {
      const position = step / 10
      const fullFrame = all[frameAtPosition(fullPositions, position)]
      const droppedFrame = dropped[frameAtPosition(droppedPositions, position)]
      expect(droppedFrame.content, `at ${position}%`).toEqual(fullFrame.content)
    }

    // (d) No frame of the deduplicated run is held with nothing changing
    // before the next: a zero gap always comes with a new board.
    dropped.forEach((frame, j) => {
      const next = dropped[j + 1]
      if (next && next.timestamp === frame.timestamp) expect(same(frame, next), `frame ${j}`).toBe(false)
    })
  })
})
