import { expect, test } from 'vitest';
import { startFrame } from './replayShare';

test('startFrame opens on the frame a link names', () => {
    expect(startFrame(143, 200)).toBe(143);
});

test('startFrame opens at the start without a frame or with a bad one', () => {
    expect(startFrame(null, 200)).toBe(0);
    expect(startFrame(undefined, 200)).toBe(0);
    expect(startFrame(Number.NaN, 200)).toBe(0);
    expect(startFrame(-4, 200)).toBe(0);
    expect(startFrame(2.5, 200)).toBe(0);
});

test('startFrame lands on the last frame when a link names one past the end', () => {
    expect(startFrame(900, 200)).toBe(199);
});

test('startFrame opens at the start of an empty game', () => {
    expect(startFrame(5, 0)).toBe(0);
});
