import { expect, test } from 'vitest';
import { clampSize, clampWindow, fromStoredPosition, parseStoredPosition, toStoredPosition, windowStorageKey } from './replayWindows';

const viewer = { width: 1000, height: 600 };
const size = { width: 320, height: 300 };

test('a window inside the viewer is left alone', () => {
    expect(clampWindow({ left: 100, top: 50 }, size, viewer)).toEqual({ left: 100, top: 50 });
});

test('a window past any edge is pulled back inside', () => {
    expect(clampWindow({ left: 900, top: 500 }, size, viewer)).toEqual({ left: 672, top: 292 });
    expect(clampWindow({ left: -40, top: -10 }, size, viewer)).toEqual({ left: 8, top: 8 });
});

test('a window larger than the viewer pins to the top left edge', () => {
    expect(clampWindow({ left: 50, top: 50 }, { width: 2000, height: 900 }, viewer)).toEqual({ left: 8, top: 8 });
});

test('stored positions round trip through viewer fractions', () => {
    const stored = toStoredPosition({ left: 250, top: 150 }, viewer);

    expect(stored).toEqual({ x: 0.25, y: 0.25 });
    expect(fromStoredPosition(stored, { width: 2000, height: 1200 })).toEqual({ left: 500, top: 300 });
});

test('unreadable stored values are rejected', () => {
    for (const raw of [null, '', '{', '[]', '{"x":"a","y":0.1}', '{"x":2,"y":0.5}', '{"x":0.1}']) {
        expect(parseStoredPosition(raw), String(raw)).toBe(null);
    }

    expect(parseStoredPosition('{"x":0.5,"y":0}')).toEqual({ x: 0.5, y: 0 });
});

test('a stored size comes back when it is a positive number pair', () => {
    expect(parseStoredPosition('{"x":0.5,"y":0.1,"width":400,"height":300}')).toEqual({ x: 0.5, y: 0.1, width: 400, height: 300 });
    expect(parseStoredPosition('{"x":0.5,"y":0.1,"width":-4,"height":300}')).toEqual({ x: 0.5, y: 0.1 });
    expect(parseStoredPosition('{"x":0.5,"y":0.1,"width":"big"}')).toEqual({ x: 0.5, y: 0.1 });
});

test('a resized window keeps a minimum and stays inside the viewer', () => {
    const min = { width: 220, height: 160 };

    expect(clampSize({ width: 100, height: 50 }, { left: 10, top: 10 }, min, viewer)).toEqual({ width: 220, height: 160 });
    expect(clampSize({ width: 5000, height: 5000 }, { left: 100, top: 50 }, min, viewer)).toEqual({ width: 892, height: 542 });
    expect(clampSize({ width: 400, height: 300 }, { left: 100, top: 50 }, min, viewer)).toEqual({ width: 400, height: 300 });
});

test('keys are per side and zone, with phones kept apart', () => {
    expect(windowStorageKey('opponent', 'Graveyard', false)).toBe('mymtgo-replay:zone-window:opponent:Graveyard');
    expect(windowStorageKey('you', 'Exile', true)).toBe('mymtgo-replay:zone-window:compact:you:Exile');
});
