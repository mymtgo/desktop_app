import { expect, test } from 'vitest';
import { clockTimeline, clocksAt, frameTimes, playheadTime, timeOfDay } from './replayTimeline';

const at = (frames: ReturnType<typeof frame>[], timeline: ReturnType<typeof clockTimeline>, index: number, progress: number) =>
    clocksAt(frames, timeline, index, playheadTime(timeline.times, index, progress));

function frame(timestamp: string, clocks: [number | undefined, number | undefined]) {
    return {
        timestamp,
        content: {
            Players: [
                { Id: 1, Name: 'Hero', Life: 20, HandCount: 7, TimeLeft: clocks[0] },
                { Id: 2, Name: 'Villain', Life: 20, HandCount: 7, TimeLeft: clocks[1] },
            ],
            Cards: [],
        },
    };
}

test('frame times count from the first frame and wrap past midnight', () => {
    const frames = [frame('23:59:58', [0, 0]), frame('23:59:59', [0, 0]), frame('00:00:02', [0, 0])];

    expect(frameTimes(frames)).toEqual([0, 1000, 4000]);
});

test('a think booked on a later frame runs down over the frames before it', () => {
    // Villain thinks for 16s; MTGO only books it when the frame at 00:00:20 lands.
    const frames = [frame('00:00:00', [1500000, 1500000]), frame('00:00:10', [1500000, 1500000]), frame('00:00:20', [1500000, 1484000])];
    const timeline = clockTimeline(frames);

    expect(at(frames, timeline, 0, 0).get(2)).toBe(1500000);
    expect(at(frames, timeline, 0, 0.5).get(2)).toBe(1499000);
    expect(at(frames, timeline, 1, 0).get(2)).toBe(1494000);
    expect(at(frames, timeline, 1, 0.5).get(2)).toBe(1489000);
    expect(at(frames, timeline, 2, 0).get(2)).toBe(1484000);
    expect(at(frames, timeline, 1, 0.5).get(1)).toBe(1500000);
});

test('a booked think lands on its recorded value at its frame', () => {
    const frames = [frame('00:00:00', [60000, 60000]), frame('00:00:00', [60000, 60000]), frame('00:00:00', [45000, 60000])];
    const timeline = clockTimeline(frames);

    expect(at(frames, timeline, 2, 0).get(1)).toBe(45000);
});

test('a clock never runs twice at once for the same player', () => {
    // The second 10s drop arrives 4s after the first, so it can only run over those 4s.
    const frames = [frame('00:00:00', [60000, 60000]), frame('00:00:10', [50000, 60000]), frame('00:00:14', [40000, 60000])];
    const timeline = clockTimeline(frames);

    expect(at(frames, timeline, 1, 0).get(1)).toBe(50000);
    expect(at(frames, timeline, 1, 0.5).get(1)).toBe(45000);
    expect(at(frames, timeline, 2, 0).get(1)).toBe(40000);
});

test('time added to a clock lands at once', () => {
    const frames = [frame('00:00:00', [60000, 60000]), frame('00:00:10', [70000, 60000])];
    const timeline = clockTimeline(frames);

    expect(at(frames, timeline, 0, 0.9).get(1)).toBe(60000);
    expect(at(frames, timeline, 1, 0).get(1)).toBe(70000);
});

test('a frame with no recorded clock shows none for that player', () => {
    const frames = [frame('00:00:00', [60000, undefined]), frame('00:00:10', [60000, undefined])];

    expect(at(frames, clockTimeline(frames), 0, 0).has(2)).toBe(false);
});

test('clocks count down whole seconds, rounded up', () => {
    const frames = [frame('00:00:00', [60000, 60000]), frame('00:00:10', [50000, 60000])];

    expect(at(frames, clockTimeline(frames), 0, 0.05).get(1)).toBe(60000);
    expect(at(frames, clockTimeline(frames), 0, 0.1).get(1)).toBe(59000);
});

test('the time of day runs on from the first frame, past midnight too', () => {
    const frames = [frame('2026-09-25T23:59:58Z', [0, 0])];

    expect(timeOfDay(frames, 0)).toBe('23:59:58');
    expect(timeOfDay(frames, 3500)).toBe('00:00:01');
    expect(timeOfDay([frame('', [0, 0])], 1000)).toBe('');
});
